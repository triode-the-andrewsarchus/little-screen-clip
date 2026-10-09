// SPDX-License-Identifier: GPL-2.0-or-later
// Independently implemented indexed video timeline generator using FFmpeg APIs.
// Timelens source is not used by this helper. See README.md for
// licensing/builds.
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <libavutil/mem.h>
#include <sys/stat.h>
#include <unistd.h>
#ifdef __linux__
#include <linux/magic.h>
#include <sys/vfs.h>
#endif
#include <libavcodec/avcodec.h>
#include <libavformat/avformat.h>
#include <libavutil/hwcontext.h>
#include <libavutil/time.h>
#include <libswscale/swscale.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#ifdef VIDEO_URING
extern void *vs_io_new(int fd);
extern void vs_io_prefetch(void *reader, const int64_t *offsets, size_t count);
extern int vs_io_read(void *reader, uint64_t offset, uint8_t *output,
                      size_t count);
extern void vs_io_free(void *reader);
#endif

#define HEIGHT 64
#define MAX_WIDTH 16384
#define MAX_WORKERS 16

#ifdef VIDEO_GPU_AVERAGE
#include "gpu_average.h"
#endif

typedef struct {
  AVFormatContext *format;
  AVIOContext *io;
  int input_fd, input_open;
  int64_t input_position, input_size;
  void *aligned;
#ifdef VIDEO_URING
  void *reader;
#endif
  AVCodecContext *codec;
  AVPacket *packet;
  AVFrame *frame, *host;
  struct SwsContext *scale;
  int stream, average_enabled, profile;
  int64_t decode_us, transfer_us, scale_us, average_us;
  int64_t read_us, read_bytes, read_calls, open_us;
  int64_t seek_us, seek_read_us, packet_us, packet_read_us;
  int64_t reset_us, send_us, drain_us, receive_us, ready_us;
  int averaged, downloaded;
#ifdef VIDEO_GPU_AVERAGE
  GpuAverage average;
#endif
} Decoder;

typedef struct {
  int64_t timestamp, position;
  int from, to;
} Sample;

typedef struct {
  pthread_mutex_t mutex;
  pthread_cond_t changed;
  uint8_t *known;
  int active, coarse_pending;
} Progress;

typedef struct {
  const char *path;
  int stream, width, begin, end, failed, uring;
  AVBufferRef *device;
  const Sample *samples;
  uint8_t *pixels;
  Progress *progress;
} Job;

static enum AVPixelFormat cuda_format(AVCodecContext *ctx,
                                      const enum AVPixelFormat *formats) {
  (void)ctx;
  for (; *formats != AV_PIX_FMT_NONE; formats++)
    if (*formats == AV_PIX_FMT_CUDA)
      return *formats;
  return AV_PIX_FMT_NONE;
}

static void close_decoder(Decoder *d) {
#ifdef VIDEO_GPU_AVERAGE
  close_average(&d->average);
#endif
  if (d->profile)
    fprintf(stderr,
            "Stages: decode_us=%lld transfer_us=%lld scale_us=%lld "
            "average_us=%lld averaged=%d downloaded=%d\n",
            (long long)d->decode_us, (long long)d->transfer_us,
            (long long)d->scale_us, (long long)d->average_us, d->averaged,
            d->downloaded);
  if (d->profile)
    fprintf(stderr,
            "Pipeline: open_us=%lld read_us=%lld read_bytes=%lld "
            "read_calls=%lld seek_us=%lld seek_read_us=%lld "
            "packet_us=%lld packet_read_us=%lld reset_us=%lld "
            "send_us=%lld drain_us=%lld receive_us=%lld ready_us=%lld\n",
            (long long)d->open_us, (long long)d->read_us,
            (long long)d->read_bytes, (long long)d->read_calls,
            (long long)d->seek_us, (long long)d->seek_read_us,
            (long long)d->packet_us, (long long)d->packet_read_us,
            (long long)d->reset_us, (long long)d->send_us,
            (long long)d->drain_us, (long long)d->receive_us,
            (long long)d->ready_us);
  sws_freeContext(d->scale);
  av_frame_free(&d->host);
  av_frame_free(&d->frame);
  av_packet_free(&d->packet);
  avcodec_free_context(&d->codec);
  avformat_close_input(&d->format);
  if (d->io)
    av_freep(&d->io->buffer);
  avio_context_free(&d->io);
#ifdef VIDEO_URING
  vs_io_free(d->reader);
#endif
  if (d->input_open)
    close(d->input_fd);
  free(d->aligned);
}

// CIFS read-ahead fetched 1.21 GB for 266 MB of indexed packet reads.
// Aligned direct reads constrain network traffic to the requested blocks.
// Local files retain normal buffered I/O and their filesystem cache.
static int sync_read_input(void *opaque, uint8_t *buffer, int count) {
  Decoder *d = opaque;
  ssize_t got;

  if (!d->aligned) {
    do {
      got = pread(d->input_fd, buffer, count, d->input_position);
    } while (got < 0 && errno == EINTR);
    if (got < 0)
      return AVERROR(errno);
    d->input_position += got;
    return got ? (int)got : AVERROR_EOF;
  }
  int offset = d->input_position % 4096;
  int wanted = count < 65536 - offset ? count : 65536 - offset;
  int bytes = (wanted + offset + 4095) / 4096 * 4096;
  do {
    got = pread(d->input_fd, d->aligned, bytes, d->input_position - offset);
  } while (got < 0 && errno == EINTR);
  if (got < 0)
    return AVERROR(errno);
  if (got <= offset)
    return AVERROR_EOF;
  int copied = got - offset < wanted ? got - offset : wanted;
  memcpy(buffer, (uint8_t *)d->aligned + offset, copied);
  d->input_position += copied;
  return copied;
}

static int read_input_data(void *opaque, uint8_t *buffer, int count) {
  Decoder *d = opaque;
#ifdef VIDEO_URING
  if (d->reader) {
    int read = vs_io_read(d->reader, d->input_position, buffer, count);
    if (read >= 0) {
      d->input_position += read;
      return read ? read : AVERROR_EOF;
    }
    vs_io_free(d->reader);
    d->reader = NULL;
  }
#endif
  return sync_read_input(d, buffer, count);
}

// Measure actual AVIO reads independently, including reads nested in seeks.
static int read_input(void *opaque, uint8_t *buffer, int count) {
  Decoder *d = opaque;
  if (!d->profile)
    return read_input_data(opaque, buffer, count);
  int64_t started = av_gettime_relative();
  int result = read_input_data(opaque, buffer, count);
  d->read_us += av_gettime_relative() - started;
  d->read_bytes += result > 0 ? result : 0;
  d->read_calls++;
  return result;
}

static int64_t seek_input(void *opaque, int64_t offset, int whence) {
  Decoder *d = opaque;
  if (whence == AVSEEK_SIZE)
    return d->input_size;
  whence &= ~AVSEEK_FORCE;
  int64_t base = 0;
  if (whence == SEEK_CUR)
    base = d->input_position;
  else if (whence == SEEK_END)
    base = d->input_size;
  else if (whence != SEEK_SET)
    return AVERROR(EINVAL);
  if (offset > INT64_MAX - base)
    return AVERROR(EINVAL);
  offset += base;
  if (offset < 0)
    return AVERROR(EINVAL);
  d->input_position = offset;
  return offset;
}

static void prefer_direct_input(Decoder *d) {
#ifdef __linux__
  struct statfs filesystem;
  if (fstatfs(d->input_fd, &filesystem) ||
      (filesystem.f_type != CIFS_SUPER_MAGIC &&
       filesystem.f_type != SMB2_SUPER_MAGIC))
    return;
  if (posix_memalign(&d->aligned, 4096, 65536)) {
    d->aligned = NULL;
    return;
  }
  int flags = fcntl(d->input_fd, F_GETFL);
  if (flags < 0 || fcntl(d->input_fd, F_SETFL, flags | O_DIRECT) < 0) {
    free(d->aligned);
    d->aligned = NULL;
  }
#else
  (void)d;
#endif
}

static int open_input(Decoder *d, const char *path) {
  d->input_fd = open(path, O_RDONLY | O_CLOEXEC);
  if (d->input_fd < 0)
    return -1;
  d->input_open = 1;
  struct stat st;
  if (fstat(d->input_fd, &st))
    return -1;
  d->input_size = st.st_size;
  prefer_direct_input(d);
  uint8_t *buffer = av_malloc(32768);
  if (!buffer)
    return -1;
  d->io = avio_alloc_context(buffer, 32768, 0, d, read_input, NULL, seek_input);
  if (!d->io) {
    av_free(buffer);
    return -1;
  }
  d->format = avformat_alloc_context();
  if (!d->format)
    return -1;
  d->format->pb = d->io;
  d->format->flags |= AVFMT_FLAG_CUSTOM_IO;
  return avformat_open_input(&d->format, path, NULL, NULL);
}

static int open_format(Decoder *d, const char *path, int ordinal) {
  if (open_input(d, path) < 0 || avformat_find_stream_info(d->format, NULL) < 0)
    return -1;
  d->stream = -1;
  for (unsigned i = 0; i < d->format->nb_streams; i++) {
    AVStream *s = d->format->streams[i];
    if (s->codecpar->codec_type == AVMEDIA_TYPE_VIDEO && ordinal-- == 0)
      d->stream = i;
  }
  if (d->stream < 0)
    return -1;
  for (unsigned i = 0; i < d->format->nb_streams; i++)
    if ((int)i != d->stream)
      d->format->streams[i]->discard = AVDISCARD_ALL;
  return 0;
}

static int open_decoder(Decoder *d, const char *path, int ordinal,
                        AVBufferRef *device) {
  d->profile = getenv("VIDEO_STRIP_PROFILE") != NULL;
  const char *average = getenv("VIDEO_STRIP_GPU_AVERAGE");
  d->average_enabled = !average || strcmp(average, "0");
  if (open_format(d, path, ordinal) < 0)
    return -1;
  const AVCodec *codec =
      avcodec_find_decoder(d->format->streams[d->stream]->codecpar->codec_id);
  if (!codec || !(d->codec = avcodec_alloc_context3(codec)) ||
      avcodec_parameters_to_context(
          d->codec, d->format->streams[d->stream]->codecpar) < 0)
    return -1;
  d->codec->thread_count = 1;
  d->codec->flags |= AV_CODEC_FLAG_LOW_DELAY;
  if (device) {
    d->codec->hw_device_ctx = av_buffer_ref(device);
    if (!d->codec->hw_device_ctx)
      return -1;
    d->codec->get_format = cuda_format;
  }
  if (avcodec_open2(d->codec, codec, NULL) < 0)
    return -1;
  d->packet = av_packet_alloc();
  d->frame = av_frame_alloc();
  d->host = av_frame_alloc();
  return d->packet && d->frame && d->host ? 0 : -1;
}

// Cues/sample tables are loaded lazily by some demuxers. Copy timestamps before
// seeking again: av_read_frame can grow/reallocate the demuxer's index entries.
typedef struct {
  int64_t timestamp, position;
} Key;

static Key *key_index(Decoder *d, int *count) {
  av_seek_frame(d->format, d->stream, INT64_MAX, AVSEEK_FLAG_BACKWARD);
  AVStream *s = d->format->streams[d->stream];
  int entries = avformat_index_get_entries_count(s);
  if (entries <= 0 || entries > 2000000)
    return NULL;
  Key *keys = malloc((size_t)entries * sizeof(*keys));
  if (!keys)
    return NULL;
  *count = 0;
  for (int i = 0; i < entries; i++) {
    const AVIndexEntry *e = avformat_index_get_entry(s, i);
    if ((e->flags & AVINDEX_KEYFRAME) &&
        (!*count || e->timestamp > keys[*count - 1].timestamp))
      keys[(*count)++] = (Key){e->timestamp, e->pos};
  }
  if (*count < 2) {
    free(keys);
    return NULL;
  }
  return keys;
}

static int make_samples(Decoder *d, int width, int64_t duration_ms,
                        Sample *samples) {
  int count;
  Key *keys = key_index(d, &count);
  if (!keys)
    return -1;
  AVStream *s = d->format->streams[d->stream];
  int64_t start = s->start_time == AV_NOPTS_VALUE ? 0 : s->start_time;
  int index = 0, used = 0;
  for (int x = 0; x < width; x++) {
    int64_t ms = av_rescale(x, duration_ms - 1, width > 1 ? width - 1 : 1);
    int64_t target =
        start + av_rescale_q(ms, (AVRational){1, 1000}, s->time_base);
    while (index + 1 < count && keys[index + 1].timestamp <= target)
      index++;
    int nearest = index;
    if (index + 1 < count &&
        keys[index + 1].timestamp - target < target - keys[index].timestamp)
      nearest++;
    int64_t timestamp = keys[nearest].timestamp;
    if (!used || samples[used - 1].timestamp != timestamp)
      samples[used++] = (Sample){timestamp, keys[nearest].position, x, x + 1};
    else
      samples[used - 1].to = x + 1;
  }
  free(keys);
  return used;
}

static int read_key_packet(Decoder *d, int64_t timestamp) {
  for (int i = 0; i < 4096; i++) {
    av_packet_unref(d->packet);
    if (av_read_frame(d->format, d->packet) < 0)
      return -1;
    if (d->packet->stream_index != d->stream ||
        !(d->packet->flags & AV_PKT_FLAG_KEY))
      continue;
    int64_t pts =
        d->packet->pts != AV_NOPTS_VALUE ? d->packet->pts : d->packet->dts;
    if (pts == AV_NOPTS_VALUE || pts >= timestamp)
      return 0;
  }
  return -1;
}

static int decode_keyframe(Decoder *d, int64_t timestamp) {
  // MP4 index entries already identify an exact key sample; ANY prevents a
  // presentation/decode timestamp offset from seeking to the preceding GOP.
  int flags = AVSEEK_FLAG_BACKWARD;
  if (strstr(d->format->iformat->name, "mov"))
    flags |= AVSEEK_FLAG_ANY;
  int64_t started = av_gettime_relative(), reads = d->read_us;
  int result = av_seek_frame(d->format, d->stream, timestamp, flags);
  d->seek_us += av_gettime_relative() - started;
  d->seek_read_us += d->read_us - reads;
  if (result < 0)
    return -1;
  started = av_gettime_relative();
  avcodec_flush_buffers(d->codec);
  av_frame_unref(d->frame);
  av_frame_unref(d->host);
  d->reset_us += av_gettime_relative() - started;
  started = av_gettime_relative();
  reads = d->read_us;
  result = read_key_packet(d, timestamp);
  d->packet_us += av_gettime_relative() - started;
  d->packet_read_us += d->read_us - reads;
  if (result < 0)
    return -1;
  started = av_gettime_relative();
  result = avcodec_send_packet(d->codec, d->packet);
  d->send_us += av_gettime_relative() - started;
  if (result < 0)
    return -1;
  // Draining also returns reordered/open-GOP pictures that immediate output
  // misses. Keep submission and frame retrieval separate for profiling.
  started = av_gettime_relative();
  result = avcodec_send_packet(d->codec, NULL);
  d->drain_us += av_gettime_relative() - started;
  if (result < 0)
    return -1;
  started = av_gettime_relative();
  result = avcodec_receive_frame(d->codec, d->frame);
  d->receive_us += av_gettime_relative() - started;
  if (result < 0)
    return -1;
#ifdef VIDEO_GPU_AVERAGE
  if (d->profile && getenv("VIDEO_STRIP_PROFILE_SYNC") &&
      d->frame->format == AV_PIX_FMT_CUDA) {
    started = av_gettime_relative();
    result = wait_decoded_frame(d->frame);
    d->ready_us += av_gettime_relative() - started;
    if (result < 0)
      return -1;
  }
#endif
  return d->frame->decode_error_flags ||
                 (d->frame->flags & AV_FRAME_FLAG_CORRUPT)
             ? -1
             : 0;
}

// The complete-frame fallback owns transfer/scaling; GPU reduction bypasses it.
static int cpu_column(Decoder *d, uint8_t *column) {
  AVFrame *frame = d->frame;
  int64_t started = av_gettime_relative();
  if (frame->format == AV_PIX_FMT_CUDA) {
    if (av_hwframe_transfer_data(d->host, frame, 0) < 0)
      return -1;
    frame = d->host;
    d->downloaded++;
  }
  d->transfer_us += av_gettime_relative() - started;
  started = av_gettime_relative();
  d->scale = sws_getCachedContext(d->scale, frame->width, frame->height,
                                  frame->format, 1, HEIGHT, AV_PIX_FMT_RGB24,
                                  SWS_AREA, NULL, NULL, NULL);
  if (!d->scale)
    return -1;
  uint8_t *output[4] = {column};
  int strides[4] = {3};
  int result = sws_scale(d->scale, (const uint8_t *const *)frame->data,
                         frame->linesize, 0, frame->height, output, strides);
  d->scale_us += av_gettime_relative() - started;
  return result == HEIGHT ? 0 : -1;
}

static int key_column(Decoder *d, int64_t timestamp, uint8_t *column) {
  int64_t started = av_gettime_relative();
  if (decode_keyframe(d, timestamp) < 0)
    return -1;
  d->decode_us += av_gettime_relative() - started;
#ifdef VIDEO_GPU_AVERAGE
  if (d->frame->format == AV_PIX_FMT_CUDA && d->average_enabled) {
    started = av_gettime_relative();
    int result = gpu_column(&d->average, d->frame, &d->scale, column);
    d->average_us += av_gettime_relative() - started;
    if (!result) {
      d->averaged++;
      return 0;
    }
    fprintf(stderr, "GPU averaging unavailable; using full-frame download.\n");
    d->average_enabled = 0;
  }
#endif
  return cpu_column(d, column);
}

static int render_sample(Job *job, Decoder *d, int i) {
#ifdef VIDEO_URING
  if (d->reader) {
    int64_t offsets[4];
    int ahead = job->end - i < 4 ? job->end - i : 4;
    for (int j = 0; j < ahead; j++)
      offsets[j] = job->samples[i + j].position;
    vs_io_prefetch(d->reader, offsets, ahead);
  }
#endif
  uint8_t column[HEIGHT * 3 + AV_INPUT_BUFFER_PADDING_SIZE];
  const Sample *sample = &job->samples[i];
  if (key_column(d, sample->timestamp, column) < 0)
    return -1;
  if (job->progress)
    pthread_mutex_lock(&job->progress->mutex);
  for (int y = 0; y < HEIGHT; y++)
    for (int x = sample->from; x < sample->to; x++)
      memcpy(job->pixels + ((size_t)y * job->width + x) * 3, column + y * 3, 3);
  if (job->progress) {
    job->progress->known[i] = 1;
    pthread_mutex_unlock(&job->progress->mutex);
  }
  return 0;
}

static void render_progressive(Job *job, Decoder *d) {
  // Eight anchors per worker give an early whole-video strip. Decoders remain
  // open, and the dense pass skips these exact samples rather than doing them twice.
  int length = job->end - job->begin;
  int anchors[8], count = length < 8 ? length : 8;
  for (int j = 0; j < count; j++) {
    anchors[j] = job->begin + j * (length - 1) / (count > 1 ? count - 1 : 1);
    if (!job->failed)
      job->failed = render_sample(job, d, anchors[j]) < 0;
  }
  pthread_mutex_lock(&job->progress->mutex);
  job->progress->coarse_pending--;
  pthread_cond_signal(&job->progress->changed);
  pthread_mutex_unlock(&job->progress->mutex);
  int next = 0;
  for (int i = job->begin; i < job->end && !job->failed; i++) {
    if (next < count && i == anchors[next]) {
      next++;
      continue;
    }
    job->failed = render_sample(job, d, i) < 0;
  }
}

static void *render(void *arg) {
  Job *job = arg;
  Decoder d = {0};
  int64_t opened = av_gettime_relative();
  job->failed = open_decoder(&d, job->path, job->stream, job->device) < 0;
  d.open_us = av_gettime_relative() - opened;
#ifdef VIDEO_URING
  if (!job->failed && job->uring)
    d.reader = vs_io_new(d.input_fd);
#endif
  if (job->progress)
    render_progressive(job, &d);
  else
    for (int i = job->begin; i < job->end && !job->failed; i++)
      job->failed = render_sample(job, &d, i) < 0;
  close_decoder(&d);
  if (job->progress) {
    pthread_mutex_lock(&job->progress->mutex);
    job->progress->active--;
    pthread_cond_signal(&job->progress->changed);
    pthread_mutex_unlock(&job->progress->mutex);
  }
  return NULL;
}

static int write_pixels(const char *path, const uint8_t *pixels, int width) {
  FILE *file = fopen(path, "wb");
  if (!file)
    return -1;
  int ok =
      fprintf(file, "P6\n%d %d\n255\n", width, HEIGHT) > 0 &&
      fwrite(pixels, 3, (size_t)width * HEIGHT, file) == (size_t)width * HEIGHT;
  if (fclose(file) != 0)
    ok = 0;
  return ok ? 0 : -1;
}

// Fill only unknown ranges from known source columns in this immutable snapshot.
static void fill_progress(uint8_t *snapshot, const uint8_t *known,
                          const Sample *samples, int count, int width) {
  int nearest = -1, next = 0;
  for (int i = 0; i < count; i++) {
    if (known[i])
      nearest = i;
    if (next <= i) {
      next = i + 1;
      while (next < count && !known[next])
        next++;
    }
    int use = nearest;
    if (next < count &&
        (use < 0 || samples[next].from - samples[i].from <
                        samples[i].from - samples[use].from))
      use = next;
    if (known[i] || use < 0)
      continue;
    for (int y = 0; y < HEIGHT; y++)
      for (int x = samples[i].from; x < samples[i].to; x++)
        memcpy(snapshot + ((size_t)y * width + x) * 3,
               snapshot + ((size_t)y * width + samples[use].from) * 3, 3);
  }
}

// Publish immutable snapshots by rename; readers never observe a partial PPM.
// Unknown columns use the nearest completed sample, so the strip has no holes.
static int publish_progress(const char *path, Progress *progress,
                            const Sample *samples, int count,
                            const uint8_t *pixels, int width) {
  size_t bytes = (size_t)width * HEIGHT * 3;
  uint8_t *snapshot = malloc(bytes), *known = malloc(count);
  char *temporary = NULL;
  if (!snapshot || !known || asprintf(&temporary, "%s.tmp", path) < 0) {
    free(snapshot);
    free(known);
    return -1;
  }
  pthread_mutex_lock(&progress->mutex);
  memcpy(snapshot, pixels, bytes);
  memcpy(known, progress->known, count);
  pthread_mutex_unlock(&progress->mutex);
  fill_progress(snapshot, known, samples, count, width);
  int result = write_pixels(temporary, snapshot, width);
  if (!result)
    result = rename(temporary, path);
  unlink(temporary);
  free(temporary);
  free(known);
  free(snapshot);
  return result;
}

static void monitor_progress(const char *path, Progress *progress,
                             const Sample *samples, int count,
                             const uint8_t *pixels, int width) {
  int64_t started = av_gettime_relative();
  pthread_mutex_lock(&progress->mutex);
  while (progress->active && progress->coarse_pending)
    pthread_cond_wait(&progress->changed, &progress->mutex);
  int active = progress->active;
  pthread_mutex_unlock(&progress->mutex);
  int published = 0;
  while (active) {
    if (publish_progress(path, progress, samples, count, pixels, width) < 0)
      break; // A preview-write failure must not abort the final render.
    if (!published++)
      fprintf(stderr, "First progressive strip: %lld us\n",
              (long long)(av_gettime_relative() - started));
    struct timespec deadline;
    clock_gettime(CLOCK_REALTIME, &deadline);
    deadline.tv_nsec += 250000000;
    deadline.tv_sec += deadline.tv_nsec / 1000000000;
    deadline.tv_nsec %= 1000000000;
    pthread_mutex_lock(&progress->mutex);
    while (progress->active &&
           pthread_cond_timedwait(&progress->changed, &progress->mutex,
                                  &deadline) != ETIMEDOUT) {}
    active = progress->active;
    pthread_mutex_unlock(&progress->mutex);
  }
}

static int parse_options(int argc, char **argv, long *width_out,
                         long *stream_out, long long *duration_out,
                         int *gpu_out) {
  if (argc < 6) {
    fprintf(stderr, "Usage: video-strip INPUT OUTPUT WIDTH VIDEO_STREAM "
                    "DURATION_MS [--gpu] [--workers 1..16] [--io-uring] [--progressive PREVIEW]\n");
    return 2;
  }
  char *end;
  errno = 0;
  long width = strtol(argv[3], &end, 10);
  if (errno || *end || width < 1 || width > MAX_WIDTH)
    return 2;
  long stream = strtol(argv[4], &end, 10);
  if (errno || *end || stream < 0 || stream > 1024)
    return 2;
  long long duration = strtoll(argv[5], &end, 10);
  if (errno || *end || duration < 1 || duration > 604800000)
    return 2;
  *gpu_out = 0;
  *width_out = width;
  *stream_out = stream;
  *duration_out = duration;
  return 0;
}

static int render_workers(const char *path, int stream, int width, int gpu,
                          int requested_workers, int uring, int count,
                          const Sample *samples, uint8_t *pixels,
                          const char *progress_path) {
  // Share one CUDA device/context across the file's decoders. Creating a
  // context per thread becomes expensive when several visible files run.
  AVBufferRef *device = NULL;
  if (gpu &&
      av_hwdevice_ctx_create(&device, AV_HWDEVICE_TYPE_CUDA, NULL, NULL, 0) < 0)
    return 1;
  pthread_t threads[MAX_WORKERS];
  Job jobs[MAX_WORKERS];
  int workers = count < requested_workers ? count : requested_workers;
  int started = 0, failed = 0;
  Progress progress = {.mutex = PTHREAD_MUTEX_INITIALIZER,
                       .changed = PTHREAD_COND_INITIALIZER,
                       .active = workers, .coarse_pending = workers};
  // If the anchor pass would already decode every sample, publish only the final.
  if (progress_path && count > workers * 8)
    progress.known = calloc(count, 1);
  for (int i = 0; i < workers; i++) {
    jobs[i] = (Job){path,
                    stream,
                    width,
                    i * count / workers,
                    (i + 1) * count / workers,
                    0,
                    uring,
                    device,
                    samples,
                    pixels,
                    progress.known ? &progress : NULL};
    if (pthread_create(&threads[i], NULL, render, &jobs[i])) {
      failed = 1;
      break;
    }
    started++;
  }
  if (progress.known) {
    pthread_mutex_lock(&progress.mutex);
    progress.active -= workers - started;
    progress.coarse_pending -= workers - started;
    pthread_mutex_unlock(&progress.mutex);
    monitor_progress(progress_path, &progress, samples, count, pixels, width);
  }
  for (int i = 0; i < started; i++) {
    pthread_join(threads[i], NULL);
    failed |= jobs[i].failed;
  }
  free(progress.known);
  pthread_mutex_destroy(&progress.mutex);
  pthread_cond_destroy(&progress.changed);
  av_buffer_unref(&device);
  return failed;
}

static int parse_flags(int argc, char **argv, int *gpu, int *workers,
                       int *uring, const char **progress_path) {
  *workers = 4;
  *uring = 0;
  *progress_path = NULL;
  for (int i = 6; i < argc; i++) {
    if (!strcmp(argv[i], "--gpu"))
      *gpu = 1;
    else if (!strcmp(argv[i], "--progressive") && i + 1 < argc)
      *progress_path = argv[++i];
    else if (!strcmp(argv[i], "--io-uring"))
      *uring = 1;
    else if (!strcmp(argv[i], "--workers") && i + 1 < argc) {
      char *end;
      errno = 0;
      long value = strtol(argv[++i], &end, 10);
      if (errno || *end || value < 1 || value > MAX_WORKERS)
        return 2;
      *workers = value;
    } else {
      return 2;
    }
  }
  return 0;
}

// Root probing/index loading is independent of the worker decoder lifecycle.
static int prepare_samples(const char *path, int stream, int width,
                           int64_t duration, int gpu, Sample *samples) {
  Decoder d = {0};
  int count = -1;
  if (open_format(&d, path, stream) == 0) {
    const AVCodecParameters *parameters = d.format->streams[d.stream]->codecpar;
    if (gpu && parameters->codec_id == AV_CODEC_ID_H264 &&
        parameters->format != AV_PIX_FMT_YUV420P) {
      fprintf(stderr, "Unsupported H.264 CUDA pixel format; retry on CPU.\n");
    } else {
      count = make_samples(&d, width, duration, samples);
    }
  }
  close_decoder(&d);
  return count;
}

int main(int argc, char **argv) {
  long width, stream;
  long long duration;
  int gpu, workers, uring;
  const char *progress_path;
  if (parse_options(argc, argv, &width, &stream, &duration, &gpu) ||
      parse_flags(argc, argv, &gpu, &workers, &uring, &progress_path))
    return 2;
  av_log_set_level(AV_LOG_ERROR);
  Sample *samples = calloc(width, sizeof(*samples));
  uint8_t *pixels = calloc((size_t)width * HEIGHT, 3);
  int count = samples && pixels ? prepare_samples(argv[1], stream, width,
                                                  duration, gpu, samples)
                                : -1;
  if (count < 1) {
    fprintf(stderr,
            "No usable keyframe index; use streaming decoder fallback.\n");
    free(samples);
    free(pixels);
    return 1;
  }
  int failed = render_workers(argv[1], stream, width, gpu, workers, uring,
                              count, samples, pixels, progress_path);
  if (!failed)
    failed = write_pixels(argv[2], pixels, width) != 0;
  fprintf(stderr, "Indexed samples: %d; columns: %ld; failed: %d\n", count,
          width, failed);
  free(samples);
  free(pixels);
  return failed ? 1 : 0;
}
