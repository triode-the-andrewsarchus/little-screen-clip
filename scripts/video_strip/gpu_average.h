// SPDX-License-Identifier: GPL-2.0-or-later
// Included only by the standalone GPL helper, never the application.
#include "average_ptx.h"
#include <dlfcn.h>
#include <ffnvcodec/dynlink_cuda.h>
#include <libavutil/hwcontext_cuda.h>

typedef struct {
  void *library;
  tcuCtxPushCurrent_v2 *push;
  tcuCtxPopCurrent_v2 *pop;
  tcuModuleLoadData *load;
  tcuModuleUnload *unload;
  tcuModuleGetFunction *function;
  tcuMemAlloc_v2 *alloc;
  tcuMemFree_v2 *free;
  tcuLaunchKernel *launch;
  tcuMemcpyDtoHAsync_v2 *copy;
  tcuStreamSynchronize *sync;
  int ready;
} Driver;

static Driver driver;
static pthread_once_t driver_once = PTHREAD_ONCE_INIT;

static void load_driver(void) {
  driver.library = dlopen("libcuda.so.1", RTLD_NOW | RTLD_LOCAL);
  if (!driver.library)
    return;
#define LOAD(member, symbol)                                                   \
  driver.member = (void *)dlsym(driver.library, symbol);                       \
  if (!driver.member)                                                          \
  return
  LOAD(push, "cuCtxPushCurrent_v2");
  LOAD(pop, "cuCtxPopCurrent_v2");
  LOAD(load, "cuModuleLoadData");
  LOAD(unload, "cuModuleUnload");
  LOAD(function, "cuModuleGetFunction");
  LOAD(alloc, "cuMemAlloc_v2");
  LOAD(free, "cuMemFree_v2");
  LOAD(launch, "cuLaunchKernel");
  LOAD(copy, "cuMemcpyDtoHAsync_v2");
  LOAD(sync, "cuStreamSynchronize");
#undef LOAD
  driver.ready = 1;
}

// Diagnostic-only barrier: attribute pending frame work before reduction.
static int wait_decoded_frame(AVFrame *frame) {
  pthread_once(&driver_once, load_driver);
  if (!driver.ready)
    return -1;
  AVHWFramesContext *frames = (void *)frame->hw_frames_ctx->data;
  AVCUDADeviceContext *device = frames->device_ctx->hwctx;
  if (driver.push(device->cuda_ctx) != CUDA_SUCCESS)
    return -1;
  int failed = driver.sync(device->stream) != CUDA_SUCCESS;
  CUcontext popped;
  driver.pop(&popped);
  return failed ? -1 : 0;
}

typedef struct {
  CUcontext context;
  CUstream stream;
  CUmodule module;
  CUfunction kernel;
  CUdeviceptr output;
  int slices;
  unsigned short column[3 * HEIGHT + AV_INPUT_BUFFER_PADDING_SIZE];
} GpuAverage;

static void close_average(GpuAverage *g) {
  if (!g->context || driver.push(g->context) != CUDA_SUCCESS)
    return;
  if (g->output)
    driver.free(g->output);
  if (g->module)
    driver.unload(g->module);
  CUcontext popped;
  driver.pop(&popped);
  memset(g, 0, sizeof(*g));
}

static int open_average(GpuAverage *g, AVHWFramesContext *frames) {
  pthread_once(&driver_once, load_driver);
  if (!driver.ready)
    return -1;
  AVCUDADeviceContext *device = frames->device_ctx->hwctx;
  g->context = device->cuda_ctx;
  g->stream = device->stream;
  if (driver.push(g->context) != CUDA_SUCCESS)
    return -1;
  const char *slices = getenv("VIDEO_STRIP_GPU_SLICES");
  g->slices = slices && !strcmp(slices, "5") ? 5 : 0;
  if (slices && !strcmp(slices, "1"))
    g->slices = 1;
  const char *kernel = "area_column";
  if (g->slices == 5)
    kernel = "five_slice_column";
  if (g->slices == 1)
    kernel = "center_column";
  int failed = driver.load(&g->module, average_ptx) != CUDA_SUCCESS ||
               driver.function(&g->kernel, g->module, kernel) != CUDA_SUCCESS ||
               driver.alloc(&g->output, sizeof(g->column)) != CUDA_SUCCESS;
  CUcontext popped;
  driver.pop(&popped);
  if (failed)
    close_average(g);
  if (!failed && getenv("VIDEO_STRIP_PROFILE"))
    fprintf(stderr, "GPU reduction: %s\n", kernel);
  return failed ? -1 : 0;
}

static int average_plane(GpuAverage *g, AVFrame *frame, int plane, int ten) {
  CUdeviceptr source = (CUdeviceptr)frame->data[plane ? 1 : 0];
  int pitch = frame->linesize[plane ? 1 : 0];
  int width = plane ? (frame->width + 1) / 2 : frame->width;
  int height = plane ? (frame->height + 1) / 2 : frame->height;
  int bytes = ten ? 2 : 1;
  int step = bytes * (plane ? 2 : 1);
  int component = plane == 2 ? bytes : 0, shift = ten ? 6 : -1;
  void *args[] = {&source,    &pitch, &width,     &height, &step,
                  &component, &shift, &g->output, &plane};
  int blocks = g->slices ? 1 : HEIGHT, threads = g->slices ? 64 : 256;
  return driver.launch(g->kernel, blocks, 1, 1, threads, 1, 1, 0, g->stream,
                       args, NULL) == CUDA_SUCCESS
             ? 0
             : -1;
}

// Keep the small-column color conversion separate from CUDA
// submission/lifetime.
static int column_rgb(GpuAverage *g, int ten, struct SwsContext **scale,
                      uint8_t *column) {
  uint8_t narrow[3 * HEIGHT + AV_INPUT_BUFFER_PADDING_SIZE] = {0};
  if (!ten)
    for (int i = 0; i < 3 * HEIGHT; i++)
      narrow[i] = g->column[i];
  int bytes = ten ? 2 : 1;
  const uint8_t *base = ten ? (uint8_t *)g->column : narrow;
  const uint8_t *data[4] = {base, base + HEIGHT * bytes,
                            base + 2 * HEIGHT * bytes};
  int strides[4] = {bytes, bytes, bytes};
  *scale = sws_getCachedContext(
      *scale, 1, HEIGHT, ten ? AV_PIX_FMT_YUV444P10LE : AV_PIX_FMT_YUV444P, 1,
      HEIGHT, AV_PIX_FMT_RGB24, SWS_AREA, NULL, NULL, NULL);
  if (!*scale)
    return -1;
  uint8_t *output[4] = {column};
  int out_stride[4] = {3};
  return sws_scale(*scale, data, strides, 0, HEIGHT, output, out_stride) ==
                 HEIGHT
             ? 0
             : -1;
}

static int gpu_column(GpuAverage *g, AVFrame *frame, struct SwsContext **scale,
                      uint8_t *column) {
  AVHWFramesContext *frames = (void *)frame->hw_frames_ctx->data;
  int ten = frames->sw_format == AV_PIX_FMT_P010LE;
  if (!ten && frames->sw_format != AV_PIX_FMT_NV12)
    return -1;
  if (!g->output && open_average(g, frames))
    return -1;
  if (driver.push(g->context) != CUDA_SUCCESS)
    return -1;
  int failed = 0;
  for (int plane = 0; plane < 3 && !failed; plane++)
    failed = average_plane(g, frame, plane, ten) != 0;
  if (!failed)
    failed =
        driver.copy(g->column, g->output, 3 * HEIGHT * sizeof(unsigned short),
                    g->stream) != CUDA_SUCCESS;
  // Synchronize before stack/frame reuse, including after a failed submission.
  failed |= driver.sync(g->stream) != CUDA_SUCCESS;
  CUcontext popped;
  driver.pop(&popped);
  if (failed)
    return -1;
  return column_rgb(g, ten, scale, column);
}
