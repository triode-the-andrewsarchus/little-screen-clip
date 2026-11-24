/* SPDX-License-Identifier: GPL-2.0-or-later */
#define _POSIX_C_SOURCE 200809L
#include <libsigrokdecode/libsigrokdecode.h>
#include <errno.h>
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

enum {
    max_pins = 7,
    max_decoders = 512,
    max_samples = 4000000,
    max_runs = 128,
    max_annotations = 4000,
    max_text = 320,
    chunk_samples = 32768,
    gap_sample = 255,
};

static FILE *protocol;

struct output {
    size_t origin;
    size_t length;
    unsigned annotations;
    unsigned omitted;
};

static void die(const char *message)
{
    fprintf(stderr, "%s\n", message);
    exit(1);
}

static void check(int status, const char *operation)
{
    if (status != SRD_OK) {
        fprintf(stderr, "%s: %s\n", operation, srd_strerror(status));
        exit(1);
    }
}

static void field(const char *text)
{
    if (!text || !g_utf8_validate(text, -1, NULL))
        return;
    size_t size = strlen(text);
    size_t end = MIN(size, max_text);
    while (end < size && ((unsigned char)text[end] & 0xc0) == 0x80)
        end--;
    for (size_t i = 0; i < end; i++) {
        unsigned char byte = text[i];
        fputc(byte < 32 || byte == 127 ? ' ' : byte, protocol);
    }
}

static gboolean decoder_id(const char *id)
{
    size_t length = strlen(id);
    return length > 0 && length <= 64 &&
           strspn(id, "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_") == length;
}

static gboolean logic_decoder(const struct srd_decoder *decoder)
{
    return decoder && decoder_id(decoder->id) && decoder->channels && decoder->annotations &&
           g_slist_find_custom(decoder->inputs, "logic", (GCompareFunc)g_strcmp0) &&
           g_slist_length(decoder->channels) + g_slist_length(decoder->opt_channels) <= max_pins;
}

static struct srd_channel *channel(const struct srd_decoder *decoder, size_t index)
{
    size_t required = g_slist_length(decoder->channels);
    return index < required ? g_slist_nth_data(decoder->channels, index)
                            : g_slist_nth_data(decoder->opt_channels, index - required);
}

static void describe(const struct srd_decoder *decoder)
{
    fprintf(protocol, "decoder\t%s\t%u\t", decoder->id, (unsigned)(decoder->options != NULL));
    field(decoder->longname);
    fputc('\n', protocol);
    size_t required = g_slist_length(decoder->channels);
    for (size_t pin = 0; pin < max_pins; pin++) {
        struct srd_channel *entry = channel(decoder, pin);
        if (!entry)
            break;
        fprintf(protocol, "pin\t%s\t%u\t", entry->id, (unsigned)(pin < required));
        field(entry->name);
        fputc('\n', protocol);
    }
}

static void catalog(void)
{
    check(srd_decoder_load_all(), "Loading installed Sigrok catalog");
    fprintf(protocol, "sigrok\t1\t%s\n", srd_package_version_string_get());
    unsigned shown = 0;
    unsigned omitted = 0;
    for (const GSList *item = srd_decoder_list(); item; item = item->next) {
        const struct srd_decoder *decoder = item->data;
        if (!logic_decoder(decoder))
            continue;
        if (shown == max_decoders) {
            omitted++;
        } else {
            describe(decoder);
            shown++;
        }
    }
    fprintf(protocol, "done\t%u\n", omitted);
}

static void annotation(struct srd_proto_data *event, void *data)
{
    struct output *output = data;
    const struct srd_proto_data_annotation *ann = event->data;
    if (output->annotations == max_annotations ||
        event->start_sample > event->end_sample || event->end_sample >= output->length ||
        !ann || !ann->ann_text || !ann->ann_text[0]) {
        output->omitted += output->omitted < UINT32_MAX;
        return;
    }
    output->annotations++;
    fprintf(protocol, "ann\t%" PRIu64 "\t%" PRIu64 "\t%d\t",
            output->origin + event->start_sample, output->origin + event->end_sample, ann->ann_class);
    field(ann->ann_text[0]);
    fputc('\n', protocol);
}

static void run(const struct srd_decoder *decoder, GHashTable *pins, uint64_t rate,
                const uint8_t *samples, struct output *output)
{
    struct srd_session *session = NULL;
    check(srd_session_new(&session), "Creating decoder session");
    GHashTable *options = g_hash_table_new(g_str_hash, g_str_equal);
    struct srd_decoder_inst *instance = srd_inst_new(session, decoder->id, options);
    g_hash_table_unref(options);
    if (!instance)
        die("Could not instantiate the decoder");
    check(srd_inst_channel_set_all(instance, pins), "Mapping decoder pins");
    check(srd_session_metadata_set(session, SRD_CONF_SAMPLERATE, g_variant_new_uint64(rate)),
          "Setting sample rate");
    check(srd_pd_output_callback_add(session, SRD_OUTPUT_ANN, annotation, output),
          "Registering annotations");
    check(srd_session_start(session), "Starting decoder");
    for (size_t start = 0; start < output->length;) {
        size_t end = MIN(start + chunk_samples, output->length);
        check(srd_session_send(session, start, end, samples + start, end - start, 1),
              "Decoding logic samples");
        start = end;
    }
    check(srd_session_terminate_reset(session), "Stopping decoder");
    check(srd_session_destroy(session), "Retiring decoder");
}

static GHashTable *pin_map(const struct srd_decoder *decoder, size_t count, char **ids)
{
    size_t required = g_slist_length(decoder->channels);
    if (count != required + g_slist_length(decoder->opt_channels))
        die("Decoder pin catalog changed");
    GHashTable *pins = g_hash_table_new_full(g_str_hash, g_str_equal, NULL,
                                            (GDestroyNotify)g_variant_unref);
    for (size_t pin = 0; pin < count; pin++) {
        if (pin >= required && strcmp(ids[pin], "-") == 0)
            continue;
        if (strcmp(ids[pin], channel(decoder, pin)->id) != 0)
            die("Decoder pin catalog changed");
        g_hash_table_insert(pins, ids[pin], g_variant_ref_sink(g_variant_new_int32(pin)));
    }
    return pins;
}

static void decode(const struct srd_decoder *decoder, GHashTable *pins, uint64_t rate)
{
    uint8_t *samples = g_malloc(max_samples);
    const size_t length = fread(samples, 1, max_samples, stdin);
    if (!length || fgetc(stdin) != EOF || ferror(stdin))
        die("Invalid logic sample stream length");
    struct output output = {0};
    unsigned runs = 0;
    size_t cursor = 0;
    size_t gaps = 0;
    while (cursor < length) {
        if (samples[cursor] == gap_sample) {
            cursor++;
            gaps++;
            continue;
        }
        if (runs == max_runs || output.annotations == max_annotations)
            break;
        output.origin = cursor;
        while (cursor < length && samples[cursor] != gap_sample)
            cursor++;
        output.length = cursor - output.origin;
        run(decoder, pins, rate, samples + output.origin, &output);
        runs++;
    }
    fprintf(protocol, "done\t%zu\t%u\t%zu\n", cursor, output.omitted, gaps);
    g_free(samples);
}

static void decode_request(int argc, char **argv)
{
    if (strcmp(argv[3], srd_package_version_string_get()) != 0)
        die("libsigrokdecode version changed");
    const char *id = argv[2];
    if (!decoder_id(id))
        die("Invalid decoder identifier");
    srd_decoder_load(id);
    struct srd_decoder *decoder = srd_decoder_get_by_id(id);
    if (!decoder) {
        check(srd_decoder_load_all(), "Loading installed Sigrok catalog");
        decoder = srd_decoder_get_by_id(id);
    }
    if (!logic_decoder(decoder))
        die("Installed logic decoder is unavailable");
    char *end = NULL;
    errno = 0;
    uint64_t rate = strtoull(argv[4], &end, 10);
    if (errno || *end || rate < 1 || rate > 10000000000ULL)
        die("Invalid decoder sample rate");
    GHashTable *pins = pin_map(decoder, argc - 5, argv + 5);
    decode(decoder, pins, rate);
    g_hash_table_unref(pins);
}

int main(int argc, char **argv)
{
    /* Python decoder diagnostics cannot write to the response pipe. */
    int fd = dup(STDOUT_FILENO);
    if (fd < 0 || dup2(STDERR_FILENO, STDOUT_FILENO) < 0 ||
        !(protocol = fdopen(fd, "w")))
        die("Could not open the decoder response pipe");
    const char *directory = getenv("LITTLE_CLIP_SIGROK_DECODERS");
    check(srd_init(directory && *directory ? directory : NULL), "Initializing libsigrokdecode");
    if (argc == 2 && strcmp(argv[1], "catalog") == 0) {
        catalog();
    } else if (argc >= 6 && argc <= 5 + max_pins && strcmp(argv[1], "decode") == 0) {
        decode_request(argc, argv);
    } else {
        die("Invalid decoder request");
    }
    check(srd_exit(), "Closing libsigrokdecode");
    return fclose(protocol) == 0 ? 0 : 1;
}
