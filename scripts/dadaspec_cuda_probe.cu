// Development-only native CUDA/cuFFT probe. No application links this library.
// The ignored Rust profile supplies owned stereo PCM and validates final
// pixels.
#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cuda_runtime.h>
#include <cufft.h>
#include <stdexcept>
#include <vector>

constexpr int FFT = 2048, MELS = 30, RATE = 32000;
using Clock = std::chrono::steady_clock;
static double milliseconds(Clock::time_point start) {
  return std::chrono::duration<double, std::milli>(Clock::now() - start)
      .count();
}
static void check(cudaError_t status) {
  if (status != cudaSuccess)
    throw std::runtime_error(cudaGetErrorString(status));
}
static void fft_check(cufftResult status) {
  if (status != CUFFT_SUCCESS)
    throw std::runtime_error("cuFFT failed");
}
static double mel(double hz) {
  return hz < 1000 ? hz / (200.0 / 3)
                   : 15 + std::log(hz / 1000) * 27 / std::log(6.4);
}
static double hz(double value) {
  return value < 15 ? value * (200.0 / 3)
                    : 1000 * std::exp((value - 15) * std::log(6.4) / 27);
}

__global__ void windows(const float *pcm, const float *hann, float *output,
                        int frames, int resampled, int columns, int hop,
                        int channels) {
  size_t index = blockIdx.x * size_t(blockDim.x) + threadIdx.x;
  size_t count = size_t(channels) * columns * FFT;
  if (index >= count)
    return;
  int sample = index % FFT;
  int column = (index / FFT) % columns;
  int channel = index / (size_t(FFT) * columns);
  float source = fmaxf(0, (float(column * hop + sample) + 0.5f) *
                                  (float(frames) / float(resampled)) -
                              0.5f);
  int first = min(int(source), frames - 1);
  int last = min(first + 1, frames - 1);
  float fraction = source - first;
  float value = (1 - fraction) * pcm[size_t(channel) * frames + first] +
                fraction * pcm[size_t(channel) * frames + last];
  output[index] = (isfinite(value) ? value : 0) * hann[sample];
}
__global__ void project(const cufftComplex *spectrum, const float *filters,
                        const int *first, const int *last, float *output,
                        int columns, int files) {
  size_t index = blockIdx.x * size_t(blockDim.x) + threadIdx.x;
  if (index >= size_t(files) * columns * 2 * MELS)
    return;
  int band = index % MELS;
  int channel = (index / MELS) % 2;
  int column = (index / (MELS * 2)) % columns;
  int file = index / (size_t(MELS * 2) * columns);
  const cufftComplex *bins =
      spectrum +
      (size_t(file * 2 + channel) * columns + column) * (FFT / 2 + 1);
  float sum = 0;
  for (int bin = first[band]; bin < last[band]; ++bin) {
    cufftComplex value = bins[bin];
    sum += (value.x * value.x + value.y * value.y) *
           filters[band * (FFT / 2 + 1) + bin];
  }
  output[index] = sum;
}

struct Context {
  int frames, resampled, columns, hop, files;
  cufftHandle plan = 0;
  float *pcm = nullptr, *input = nullptr, *powers = nullptr, *hann = nullptr,
        *filters = nullptr;
  int *first = nullptr, *last = nullptr;
  cufftComplex *spectrum = nullptr;
  ~Context() {
    if (plan)
      cufftDestroy(plan);
    cudaFree(pcm);
    cudaFree(input);
    cudaFree(powers);
    cudaFree(hann);
    cudaFree(filters);
    cudaFree(first);
    cudaFree(last);
    cudaFree(spectrum);
  }
};

extern "C" void *dadaspec_probe_create(int frames, int rate, int width,
                                       int files, double *context_ms,
                                       double *setup_ms) {
  Context *context = nullptr;
  try {
    if (frames <= 0 || frames > 30000000 || rate < 8000 || width < 1 ||
        width > 4096 || files < 1 || files > 8)
      throw std::runtime_error("invalid probe dimensions");
    auto start = Clock::now();
    check(cudaFree(nullptr));
    *context_ms = milliseconds(start);
    start = Clock::now();
    context = new Context();
    auto &c = *context;
    c.frames = frames;
    c.files = files;
    c.resampled = int(std::nearbyint(double(frames) * RATE / rate));
    c.hop = std::clamp(
        (std::max(0, c.resampled - FFT) + std::max(1, width - 1) - 1) /
            std::max(1, width - 1),
        64, FFT);
    c.columns = std::max(1, (c.resampled - FFT) / c.hop + 1);
    size_t windows_count = size_t(files) * 2 * c.columns;
    check(cudaMalloc(&c.pcm, size_t(files) * 2 * frames * sizeof(float)));
    check(cudaMalloc(&c.input, windows_count * FFT * sizeof(float)));
    check(cudaMalloc(&c.spectrum,
                     windows_count * (FFT / 2 + 1) * sizeof(cufftComplex)));
    check(cudaMalloc(&c.powers, windows_count * MELS * sizeof(float)));
    check(cudaMalloc(&c.hann, FFT * sizeof(float)));
    check(cudaMalloc(&c.filters, MELS * (FFT / 2 + 1) * sizeof(float)));
    check(cudaMalloc(&c.first, MELS * sizeof(int)));
    check(cudaMalloc(&c.last, MELS * sizeof(int)));
    std::vector<float> hann(FFT), filters(MELS * (FFT / 2 + 1));
    std::vector<int> first(MELS, FFT / 2 + 1), last(MELS, 0);
    for (int i = 0; i < FFT; ++i)
      hann[i] = float(0.5 - 0.5 * std::cos(2 * std::acos(-1.0) * i / FFT));
    for (int m = 0; m < MELS; ++m) {
      double lo = hz(mel(16000) * m / (MELS + 1));
      double mid = hz(mel(16000) * (m + 1) / (MELS + 1));
      double hi = hz(mel(16000) * (m + 2) / (MELS + 1));
      for (int bin = 0; bin <= FFT / 2; ++bin) {
        double f = double(bin) * RATE / FFT;
        float weight = float(std::max(0.0, std::min((f - lo) / (mid - lo),
                                                    (hi - f) / (hi - mid)))) *
                       float(2 / (hi - lo));
        filters[m * (FFT / 2 + 1) + bin] = weight;
        if (weight > 0) {
          first[m] = std::min(first[m], bin);
          last[m] = bin + 1;
        }
      }
    }
    check(cudaMemcpy(c.hann, hann.data(), FFT * sizeof(float),
                     cudaMemcpyHostToDevice));
    check(cudaMemcpy(c.filters, filters.data(), filters.size() * sizeof(float),
                     cudaMemcpyHostToDevice));
    check(cudaMemcpy(c.first, first.data(), MELS * sizeof(int),
                     cudaMemcpyHostToDevice));
    check(cudaMemcpy(c.last, last.data(), MELS * sizeof(int),
                     cudaMemcpyHostToDevice));
    fft_check(cufftPlan1d(&c.plan, FFT, CUFFT_R2C, int(windows_count)));
    check(cudaDeviceSynchronize());
    *setup_ms = milliseconds(start);
    return context;
  } catch (const std::exception &error) {
    std::fprintf(stderr, "CUDA probe setup: %s\n", error.what());
    delete context;
    return nullptr;
  }
}
extern "C" int dadaspec_probe_run(void *handle, const float *const *channels,
                                  float *output) {
  try {
    auto &c = *static_cast<Context *>(handle);
    for (int channel = 0; channel < c.files * 2; ++channel)
      check(cudaMemcpy(c.pcm + size_t(channel) * c.frames, channels[channel],
                       c.frames * sizeof(float), cudaMemcpyHostToDevice));
    size_t count = size_t(c.files) * 2 * c.columns * FFT;
    windows<<<(count + 255) / 256, 256>>>(c.pcm, c.hann, c.input, c.frames,
                                          c.resampled, c.columns, c.hop,
                                          c.files * 2);
    check(cudaGetLastError());
    fft_check(cufftExecR2C(c.plan, c.input, c.spectrum));
    count = size_t(c.files) * 2 * c.columns * MELS;
    project<<<(count + 255) / 256, 256>>>(c.spectrum, c.filters, c.first,
                                          c.last, c.powers, c.columns, c.files);
    check(cudaGetLastError());
    check(cudaMemcpy(output, c.powers, count * sizeof(float),
                     cudaMemcpyDeviceToHost));
    check(cudaDeviceSynchronize());
    return 0;
  } catch (const std::exception &error) {
    std::fprintf(stderr, "CUDA probe run: %s\n", error.what());
    return 1;
  }
}
extern "C" int dadaspec_probe_columns(void *handle) {
  return static_cast<Context *>(handle)->columns;
}
extern "C" void dadaspec_probe_destroy(void *handle) {
  delete static_cast<Context *>(handle);
}
