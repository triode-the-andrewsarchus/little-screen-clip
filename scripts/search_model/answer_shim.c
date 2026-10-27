// Local generative answers using the same pinned llama.cpp libraries as search.
// No document paths, tools, networking, or mutable application state enter here.
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include "ggml-backend.h"
#include "llama.h"
#include "mtmd.h"
#include "mtmd-helper.h"

#define ANSWER_CONTEXT 8192
#define ANSWER_OUTPUT 768

typedef struct {
    mtmd_context * vision;
    struct llama_model * model;
    struct llama_context * context;
    const struct llama_vocab * vocab;
    struct llama_batch_ext * batch;
    ggml_backend_dev_t devices[2];
} clip_answer;

void clip_answer_close(clip_answer * answer) {
    if (!answer) return;
    if (answer->vision) mtmd_free(answer->vision);
    if (answer->batch) llama_batch_ext_free(answer->batch);
    if (answer->context) llama_free(answer->context);
    if (answer->model) llama_model_free(answer->model);
    free(answer);
}

// Backend registration/logging is initialized by the embedding shim first.
clip_answer * clip_answer_open(const char * path, const char * device) {
    clip_answer * answer = calloc(1, sizeof(*answer));
    if (!answer) return NULL;
    bool cpu = strcmp(device, "CPU") == 0;
    if (!cpu) {
        answer->devices[0] = ggml_backend_dev_by_name(device);
        if (!answer->devices[0]) { clip_answer_close(answer); return NULL; }
    }
    struct llama_model_params model = llama_model_default_params();
    model.devices = answer->devices;
    model.n_gpu_layers = cpu ? 0 : 999;
    answer->model = llama_model_load_from_file(path, model);
    if (!answer->model) { clip_answer_close(answer); return NULL; }
    char architecture[64];
    if (llama_model_meta_val_str(answer->model, "general.architecture", architecture, sizeof(architecture)) < 0
        || strcmp(architecture, "qwen35") != 0) {
        clip_answer_close(answer); return NULL;
    }
    struct llama_context_params context = llama_context_default_params();
    context.n_ctx = ANSWER_CONTEXT;
    context.n_batch = 512;
    context.n_ubatch = 512;
    context.n_seq_max = 1;
    context.n_threads = 4;
    context.n_threads_batch = 4;
    answer->context = llama_init_from_model(answer->model, context);
    if (!answer->context) { clip_answer_close(answer); return NULL; }
    answer->vocab = llama_model_get_vocab(answer->model);
    answer->batch = llama_batch_ext_init(answer->context);
    if (!answer->batch) { clip_answer_close(answer); return NULL; }
    return answer;
}

static int append(clip_answer * answer, llama_token * tokens, int used, const char * text, bool special) {
    if (used < 0 || used >= ANSWER_CONTEXT) return -1;
    int count = llama_tokenize(answer->vocab, text, (int32_t) strlen(text), tokens + used,
                               ANSWER_CONTEXT - used, false, special);
    return count < 0 ? -1 : used + count;
}

// Qwen3.5 non-thinking chat framing. Only application-owned role markers are
// parsed as special tokens; literal document/question markers remain data.
static int prompt(clip_answer * answer, llama_token * tokens, const char * system, const char * user) {
    int used = append(answer, tokens, 0, "<|im_start|>system\n", true);
    used = append(answer, tokens, used, system, false);
    used = append(answer, tokens, used, "<|im_end|>\n<|im_start|>user\n", true);
    used = append(answer, tokens, used, user, false);
    return append(answer, tokens, used, "<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n", true);
}

static int decode(clip_answer * answer, const llama_token * tokens, int count, int position) {
    llama_batch_ext_clear(answer->batch);
    for (int i = 0; i < count; i++) {
        int index = llama_batch_ext_add_token(answer->batch, 0, tokens[i]);
        llama_pos pos = position + i;
        if (index < 0 || !llama_batch_ext_set_pos(answer->batch, index, &pos)) return 1;
    }
    if (!llama_batch_ext_set_output_logits(answer->batch, count - 1, true)) return 1;
    return llama_process(answer->context, LLAMA_PROCESS_TYPE_DECODE, answer->batch);
}

static int generate(clip_answer * answer, const char * grammar, char * output, size_t size, int count) {
    struct llama_sampler * constraint = llama_sampler_init_grammar(answer->vocab, grammar, "root");
    if (!constraint) return 3;
    struct llama_sampler * sampler = llama_sampler_chain_init(llama_sampler_chain_default_params());
    llama_sampler_chain_add(sampler, llama_sampler_init_greedy());
    struct llama_sampler * fallback = llama_sampler_chain_init(llama_sampler_chain_default_params());
    llama_sampler_chain_add(fallback, constraint);
    llama_sampler_chain_add(fallback, llama_sampler_init_greedy());
    int status = 4;
    size_t used = 0;
    output[0] = '\0';
    for (int step = 0; step < ANSWER_OUTPUT; step++) {
        llama_token token = llama_sampler_sample(sampler, answer->context, -1);
        // Match the pinned common/sampling.cpp grammar-rejection path. Greedy
        // sampling is stateless: a valid maximum is also the constrained maximum.
        // Invalid maxima still use the full grammar filter, never a top-k cutoff.
        llama_token_data candidate = {token, 1.0f, 0.0f};
        llama_token_data_array single = {&candidate, 1, -1, false};
        llama_sampler_apply(constraint, &single);
        if (candidate.logit != -INFINITY) {
            llama_sampler_accept(constraint, token);
        } else {
            token = llama_sampler_sample(fallback, answer->context, -1);
        }
        if (llama_vocab_is_eog(answer->vocab, token)) { status = 0; break; }
        char piece[2048];
        int length = llama_token_to_piece(answer->vocab, token, piece, sizeof(piece), 0, false);
        if (length < 0 || used + (size_t) length >= size) break;
        memcpy(output + used, piece, (size_t) length);
        used += (size_t) length;
        output[used] = '\0';
        if (decode(answer, &token, 1, count + step)) { status = 2; break; }
    }
    llama_sampler_free(fallback); // Owns the grammar; accept it exactly once above.
    llama_sampler_free(sampler);
    return status;
}

// 0: complete; 1: context overflow; 2: decode failure; 3: invalid grammar;
// 4: output budget exhausted. Partial generated text must never be accepted.
int clip_answer_generate(clip_answer * answer, const char * system, const char * user,
                         const char * grammar, char * output, size_t size) {
    llama_token tokens[ANSWER_CONTEXT];
    int count = prompt(answer, tokens, system, user);
    if (count < 1 || count + ANSWER_OUTPUT >= ANSWER_CONTEXT || size < 1) return 1;
    llama_memory_clear(llama_get_memory(answer->context), true);
    for (int start = 0; start < count; start += 512) {
        int batch = count - start > 512 ? 512 : count - start;
        if (decode(answer, tokens + start, batch, start)) return 2;
    }
    return generate(answer, grammar, output, size, count);
}

// Projection weights are optional and loaded only for an explicit visual request.
int clip_answer_load_vision(clip_answer * answer, const char * projector) {
    if (answer->vision) return 0;
    struct mtmd_context_params params = mtmd_context_params_default();
    params.use_gpu = answer->devices[0] != NULL;
    params.device = answer->devices[0];
    params.n_threads = 4;
    params.warmup = false;
    params.image_min_tokens = 64;
    params.image_max_tokens = 1024;
    answer->vision = mtmd_init_from_file(projector, answer->model, params);
    return answer->vision ? 0 : 2;
}

static mtmd_bitmap * prepared_bitmap(clip_answer * answer, const unsigned char * png, size_t png_size) {
    if (!answer->vision || !png || png_size < 24 || png_size > 8388608
        || memcmp(png, "\x89PNG\r\n\x1a\n", 8) || memcmp(png + 12, "IHDR", 4)) return NULL;
    uint32_t width = ((uint32_t) png[16] << 24) | ((uint32_t) png[17] << 16) | ((uint32_t) png[18] << 8) | png[19];
    uint32_t height = ((uint32_t) png[20] << 24) | ((uint32_t) png[21] << 16) | ((uint32_t) png[22] << 8) | png[23];
    if (!width || !height || width > 2048 || height > 2048) return NULL;
    struct mtmd_helper_bitmap_wrapper wrapper = mtmd_helper_bitmap_init_from_buf(
        answer->vision, png, png_size, false, mtmd_helper_init_opt_default());
    mtmd_helper_video_free(wrapper.video_ctx);
    return wrapper.bitmap;
}

// Only bounded prepared PNG bytes enter this API. Source paths never enter mtmd.
int clip_answer_generate_image(clip_answer * answer, const char * system, const char * user,
                              const char * grammar, const unsigned char * png, size_t png_size,
                              char * output, size_t size) {
    if (size < 1) return 2;
    mtmd_bitmap * bitmap = prepared_bitmap(answer, png, png_size);
    mtmd_input_chunks * chunks = mtmd_input_chunks_init();
    if (!bitmap || !chunks) {
        if (bitmap) mtmd_bitmap_free(bitmap);
        if (chunks) mtmd_input_chunks_free(chunks);
        return 2;
    }
    const char * prefix = "<|im_start|>system\n";
    const char * middle = "<|im_end|>\n<|im_start|>user\n";
    const char * suffix = "<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n";
    mtmd_input_text text[] = {
        {prefix, strlen(prefix), false, true}, {system, strlen(system), false, false},
        {middle, strlen(middle), false, true}, {user, strlen(user), false, false},
        {suffix, strlen(suffix), false, true}
    };
    mtmd_input_part parts[] = {
        {&text[0], NULL}, {&text[1], NULL}, {&text[2], NULL},
        {NULL, bitmap}, {&text[3], NULL}, {&text[4], NULL}
    };
    const mtmd_input_part * pointers[] = {&parts[0], &parts[1], &parts[2], &parts[3], &parts[4], &parts[5]};
    int status = 2;
    if (mtmd_tokenize_from_parts(answer->vision, chunks, pointers, 6, false)) goto done;
    size_t tokens = 0;
    for (size_t i = 0; i < mtmd_input_chunks_size(chunks); i++)
        tokens += mtmd_input_chunk_get_n_tokens(mtmd_input_chunks_get(chunks, i));
    if (tokens + ANSWER_OUTPUT >= ANSWER_CONTEXT) { status = 1; goto done; }
    llama_memory_clear(llama_get_memory(answer->context), true);
    llama_pos position = 0;
    if (mtmd_helper_eval_chunks(answer->vision, answer->context, chunks, 0, 0, 512, true, &position)) goto done;
    status = generate(answer, grammar, output, size, position);
done:
    mtmd_input_chunks_free(chunks);
    mtmd_bitmap_free(bitmap);
    return status;
}
