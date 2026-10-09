// SPDX-License-Identifier: GPL-2.0-or-later
// One block per output row/plane. Every source pixel contributes, including
// fractional row overlap at output boundaries; this is not sparse sampling.
extern "C" __global__ void area_column(const unsigned char *source, int pitch,
                                       int width, int height, int step,
                                       int component, int shift,
                                       unsigned short *output, int plane) {
  int row = blockIdx.x, lane = threadIdx.x;
  float begin = row * (height / 64.0f), end = (row + 1) * (height / 64.0f);
  int first = (int)begin, last = (int)ceilf(end);
  float sum = 0;
  for (int index = lane; index < (last - first) * width; index += blockDim.x) {
    int y = first + index / width, x = index % width;
    if (y >= height)
      continue;
    float weight = fminf(end, y + 1.0f) - fmaxf(begin, (float)y);
    const unsigned char *pixel = source + y * pitch + x * step + component;
    unsigned int value =
        shift < 0 ? *pixel : (*(const unsigned short *)pixel >> shift);
    sum += value * weight;
  }
  __shared__ float partial[256];
  partial[lane] = sum;
  __syncthreads();
  for (int stride = 128; stride; stride /= 2) {
    if (lane < stride)
      partial[lane] += partial[lane + stride];
    __syncthreads();
  }
  if (!lane)
    output[plane * 64 + row] =
        (unsigned short)(partial[0] / (width * (end - begin)) + 0.5f);
}

// Experimental sparse mode: five single-pixel vertical columns, including both
// edges and the center. One thread owns an output row; only those five columns
// are read. Vertical area weights match the complete-frame reducer above.
extern "C" __global__ void five_slice_column(const unsigned char *source,
                                             int pitch, int width, int height,
                                             int step, int component, int shift,
                                             unsigned short *output,
                                             int plane) {
  int row = threadIdx.x;
  float begin = row * (height / 64.0f), end = (row + 1) * (height / 64.0f);
  float sum = 0;
  for (int y = (int)begin; y < (int)ceilf(end) && y < height; y++) {
    float weight = fminf(end, y + 1.0f) - fmaxf(begin, (float)y);
    for (int slice = 0; slice < 5; slice++) {
      int x = slice * (width - 1) / 4;
      const unsigned char *pixel = source + y * pitch + x * step + component;
      unsigned int value =
          shift < 0 ? *pixel : (*(const unsigned short *)pixel >> shift);
      sum += value * weight;
    }
  }
  output[plane * 64 + row] = (unsigned short)(sum / (5 * (end - begin)) + 0.5f);
}

// No averaging: one center-column pixel at each output row's vertical midpoint.
extern "C" __global__ void center_column(const unsigned char *source, int pitch,
                                         int width, int height, int step,
                                         int component, int shift,
                                         unsigned short *output, int plane) {
  int row = threadIdx.x;
  int x = width / 2, y = (2 * row + 1) * height / 128;
  const unsigned char *pixel = source + y * pitch + x * step + component;
  output[plane * 64 + row] =
      shift < 0 ? *pixel : (*(const unsigned short *)pixel >> shift);
}
