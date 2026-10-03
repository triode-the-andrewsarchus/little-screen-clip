//! SH rotation adapted from PlayCanvas SplatTransform (MIT).
//! Source: src/lib/utils/rotate-sh.ts at 435b972a97a278c655d8f7015b1355215cba0d71.
/*
Copyright (c) 2011-2026 PlayCanvas Ltd.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.

*/
use wgpu_3dgs_viewer::core::glam::{Mat3, Vec3};

pub(super) struct ShRotation([[f32; 15]; 15]);
impl ShRotation {
    pub(super) fn new(matrix: Mat3) -> Self {
        let rot = matrix.to_cols_array();
        let ksqrt03_02 = (3.0_f32 / 2.0).sqrt();
        let ksqrt01_03 = (1.0_f32 / 3.0).sqrt();
        let ksqrt02_03 = (2.0_f32 / 3.0).sqrt();
        let ksqrt04_03 = (4.0_f32 / 3.0).sqrt();
        let ksqrt01_04 = (1.0_f32 / 4.0).sqrt();
        let ksqrt03_04 = (3.0_f32 / 4.0).sqrt();
        let ksqrt01_05 = (1.0_f32 / 5.0).sqrt();
        let ksqrt03_05 = (3.0_f32 / 5.0).sqrt();
        let ksqrt06_05 = (6.0_f32 / 5.0).sqrt();
        let ksqrt08_05 = (8.0_f32 / 5.0).sqrt();
        let ksqrt09_05 = (9.0_f32 / 5.0).sqrt();
        let ksqrt01_06 = (1.0_f32 / 6.0).sqrt();
        let ksqrt05_06 = (5.0_f32 / 6.0).sqrt();
        let ksqrt03_08 = (3.0_f32 / 8.0).sqrt();
        let ksqrt05_08 = (5.0_f32 / 8.0).sqrt();
        let ksqrt09_08 = (9.0_f32 / 8.0).sqrt();
        let ksqrt05_09 = (5.0_f32 / 9.0).sqrt();
        let ksqrt08_09 = (8.0_f32 / 9.0).sqrt();
        let ksqrt01_10 = (1.0_f32 / 10.0).sqrt();
        let ksqrt03_10 = (3.0_f32 / 10.0).sqrt();
        let ksqrt01_12 = (1.0_f32 / 12.0).sqrt();
        let ksqrt04_15 = (4.0_f32 / 15.0).sqrt();
        let ksqrt01_16 = (1.0_f32 / 16.0).sqrt();
        let ksqrt15_16 = (15.0_f32 / 16.0).sqrt();
        let ksqrt01_18 = (1.0_f32 / 18.0).sqrt();
        let ksqrt01_60 = (1.0_f32 / 60.0).sqrt();
        let sh1: [[f32; 3]; 3] = [
            [rot[4], -rot[7], rot[1]],
            [-rot[5], rot[8], -rot[2]],
            [rot[3], -rot[6], rot[0]],
        ];
        let sh2: [[f32; 5]; 5] = [
            [
                ksqrt01_04
                    * (sh1[2][2] * sh1[0][0]
                        + sh1[2][0] * sh1[0][2]
                        + (sh1[0][2] * sh1[2][0] + sh1[0][0] * sh1[2][2])),
                sh1[2][1] * sh1[0][0] + sh1[0][1] * sh1[2][0],
                ksqrt03_04 * (sh1[2][1] * sh1[0][1] + sh1[0][1] * sh1[2][1]),
                sh1[2][1] * sh1[0][2] + sh1[0][1] * sh1[2][2],
                ksqrt01_04
                    * (sh1[2][2] * sh1[0][2] - sh1[2][0] * sh1[0][0]
                        + (sh1[0][2] * sh1[2][2] - sh1[0][0] * sh1[2][0])),
            ],
            [
                ksqrt01_04
                    * (sh1[1][2] * sh1[0][0]
                        + sh1[1][0] * sh1[0][2]
                        + (sh1[0][2] * sh1[1][0] + sh1[0][0] * sh1[1][2])),
                sh1[1][1] * sh1[0][0] + sh1[0][1] * sh1[1][0],
                ksqrt03_04 * (sh1[1][1] * sh1[0][1] + sh1[0][1] * sh1[1][1]),
                sh1[1][1] * sh1[0][2] + sh1[0][1] * sh1[1][2],
                ksqrt01_04
                    * (sh1[1][2] * sh1[0][2] - sh1[1][0] * sh1[0][0]
                        + (sh1[0][2] * sh1[1][2] - sh1[0][0] * sh1[1][0])),
            ],
            [
                ksqrt01_03 * (sh1[1][2] * sh1[1][0] + sh1[1][0] * sh1[1][2])
                    - ksqrt01_12
                        * (sh1[2][2] * sh1[2][0]
                            + sh1[2][0] * sh1[2][2]
                            + (sh1[0][2] * sh1[0][0] + sh1[0][0] * sh1[0][2])),
                ksqrt04_03 * sh1[1][1] * sh1[1][0]
                    - ksqrt01_03 * (sh1[2][1] * sh1[2][0] + sh1[0][1] * sh1[0][0]),
                sh1[1][1] * sh1[1][1]
                    - ksqrt01_04 * (sh1[2][1] * sh1[2][1] + sh1[0][1] * sh1[0][1]),
                ksqrt04_03 * sh1[1][1] * sh1[1][2]
                    - ksqrt01_03 * (sh1[2][1] * sh1[2][2] + sh1[0][1] * sh1[0][2]),
                ksqrt01_03 * (sh1[1][2] * sh1[1][2] - sh1[1][0] * sh1[1][0])
                    - ksqrt01_12
                        * (sh1[2][2] * sh1[2][2] - sh1[2][0] * sh1[2][0]
                            + (sh1[0][2] * sh1[0][2] - sh1[0][0] * sh1[0][0])),
            ],
            [
                ksqrt01_04
                    * (sh1[1][2] * sh1[2][0]
                        + sh1[1][0] * sh1[2][2]
                        + (sh1[2][2] * sh1[1][0] + sh1[2][0] * sh1[1][2])),
                sh1[1][1] * sh1[2][0] + sh1[2][1] * sh1[1][0],
                ksqrt03_04 * (sh1[1][1] * sh1[2][1] + sh1[2][1] * sh1[1][1]),
                sh1[1][1] * sh1[2][2] + sh1[2][1] * sh1[1][2],
                ksqrt01_04
                    * (sh1[1][2] * sh1[2][2] - sh1[1][0] * sh1[2][0]
                        + (sh1[2][2] * sh1[1][2] - sh1[2][0] * sh1[1][0])),
            ],
            [
                ksqrt01_04
                    * (sh1[2][2] * sh1[2][0] + sh1[2][0] * sh1[2][2]
                        - (sh1[0][2] * sh1[0][0] + sh1[0][0] * sh1[0][2])),
                sh1[2][1] * sh1[2][0] - sh1[0][1] * sh1[0][0],
                ksqrt03_04 * (sh1[2][1] * sh1[2][1] - sh1[0][1] * sh1[0][1]),
                sh1[2][1] * sh1[2][2] - sh1[0][1] * sh1[0][2],
                ksqrt01_04
                    * (sh1[2][2] * sh1[2][2]
                        - sh1[2][0] * sh1[2][0]
                        - (sh1[0][2] * sh1[0][2] - sh1[0][0] * sh1[0][0])),
            ],
        ];
        let sh3: [[f32; 7]; 7] = [
            [
                ksqrt01_04
                    * (sh1[2][2] * sh2[0][0]
                        + sh1[2][0] * sh2[0][4]
                        + (sh1[0][2] * sh2[4][0] + sh1[0][0] * sh2[4][4])),
                ksqrt03_02 * (sh1[2][1] * sh2[0][0] + sh1[0][1] * sh2[4][0]),
                ksqrt15_16 * (sh1[2][1] * sh2[0][1] + sh1[0][1] * sh2[4][1]),
                ksqrt05_06 * (sh1[2][1] * sh2[0][2] + sh1[0][1] * sh2[4][2]),
                ksqrt15_16 * (sh1[2][1] * sh2[0][3] + sh1[0][1] * sh2[4][3]),
                ksqrt03_02 * (sh1[2][1] * sh2[0][4] + sh1[0][1] * sh2[4][4]),
                ksqrt01_04
                    * (sh1[2][2] * sh2[0][4] - sh1[2][0] * sh2[0][0]
                        + (sh1[0][2] * sh2[4][4] - sh1[0][0] * sh2[4][0])),
            ],
            [
                ksqrt01_06 * (sh1[1][2] * sh2[0][0] + sh1[1][0] * sh2[0][4])
                    + ksqrt01_06
                        * (sh1[2][2] * sh2[1][0]
                            + sh1[2][0] * sh2[1][4]
                            + (sh1[0][2] * sh2[3][0] + sh1[0][0] * sh2[3][4])),
                sh1[1][1] * sh2[0][0] + (sh1[2][1] * sh2[1][0] + sh1[0][1] * sh2[3][0]),
                ksqrt05_08 * sh1[1][1] * sh2[0][1]
                    + ksqrt05_08 * (sh1[2][1] * sh2[1][1] + sh1[0][1] * sh2[3][1]),
                ksqrt05_09 * sh1[1][1] * sh2[0][2]
                    + ksqrt05_09 * (sh1[2][1] * sh2[1][2] + sh1[0][1] * sh2[3][2]),
                ksqrt05_08 * sh1[1][1] * sh2[0][3]
                    + ksqrt05_08 * (sh1[2][1] * sh2[1][3] + sh1[0][1] * sh2[3][3]),
                sh1[1][1] * sh2[0][4] + (sh1[2][1] * sh2[1][4] + sh1[0][1] * sh2[3][4]),
                ksqrt01_06 * (sh1[1][2] * sh2[0][4] - sh1[1][0] * sh2[0][0])
                    + ksqrt01_06
                        * (sh1[2][2] * sh2[1][4] - sh1[2][0] * sh2[1][0]
                            + (sh1[0][2] * sh2[3][4] - sh1[0][0] * sh2[3][0])),
            ],
            [
                ksqrt04_15 * (sh1[1][2] * sh2[1][0] + sh1[1][0] * sh2[1][4])
                    + ksqrt01_05 * (sh1[0][2] * sh2[2][0] + sh1[0][0] * sh2[2][4])
                    - ksqrt01_60
                        * (sh1[2][2] * sh2[0][0] + sh1[2][0] * sh2[0][4]
                            - (sh1[0][2] * sh2[4][0] + sh1[0][0] * sh2[4][4])),
                ksqrt08_05 * sh1[1][1] * sh2[1][0] + ksqrt06_05 * sh1[0][1] * sh2[2][0]
                    - ksqrt01_10 * (sh1[2][1] * sh2[0][0] - sh1[0][1] * sh2[4][0]),
                sh1[1][1] * sh2[1][1] + ksqrt03_04 * sh1[0][1] * sh2[2][1]
                    - ksqrt01_16 * (sh1[2][1] * sh2[0][1] - sh1[0][1] * sh2[4][1]),
                ksqrt08_09 * sh1[1][1] * sh2[1][2] + ksqrt02_03 * sh1[0][1] * sh2[2][2]
                    - ksqrt01_18 * (sh1[2][1] * sh2[0][2] - sh1[0][1] * sh2[4][2]),
                sh1[1][1] * sh2[1][3] + ksqrt03_04 * sh1[0][1] * sh2[2][3]
                    - ksqrt01_16 * (sh1[2][1] * sh2[0][3] - sh1[0][1] * sh2[4][3]),
                ksqrt08_05 * sh1[1][1] * sh2[1][4] + ksqrt06_05 * sh1[0][1] * sh2[2][4]
                    - ksqrt01_10 * (sh1[2][1] * sh2[0][4] - sh1[0][1] * sh2[4][4]),
                ksqrt04_15 * (sh1[1][2] * sh2[1][4] - sh1[1][0] * sh2[1][0])
                    + ksqrt01_05 * (sh1[0][2] * sh2[2][4] - sh1[0][0] * sh2[2][0])
                    - ksqrt01_60
                        * (sh1[2][2] * sh2[0][4]
                            - sh1[2][0] * sh2[0][0]
                            - (sh1[0][2] * sh2[4][4] - sh1[0][0] * sh2[4][0])),
            ],
            [
                ksqrt03_10 * (sh1[1][2] * sh2[2][0] + sh1[1][0] * sh2[2][4])
                    - ksqrt01_10
                        * (sh1[2][2] * sh2[3][0]
                            + sh1[2][0] * sh2[3][4]
                            + (sh1[0][2] * sh2[1][0] + sh1[0][0] * sh2[1][4])),
                ksqrt09_05 * sh1[1][1] * sh2[2][0]
                    - ksqrt03_05 * (sh1[2][1] * sh2[3][0] + sh1[0][1] * sh2[1][0]),
                ksqrt09_08 * sh1[1][1] * sh2[2][1]
                    - ksqrt03_08 * (sh1[2][1] * sh2[3][1] + sh1[0][1] * sh2[1][1]),
                sh1[1][1] * sh2[2][2]
                    - ksqrt01_03 * (sh1[2][1] * sh2[3][2] + sh1[0][1] * sh2[1][2]),
                ksqrt09_08 * sh1[1][1] * sh2[2][3]
                    - ksqrt03_08 * (sh1[2][1] * sh2[3][3] + sh1[0][1] * sh2[1][3]),
                ksqrt09_05 * sh1[1][1] * sh2[2][4]
                    - ksqrt03_05 * (sh1[2][1] * sh2[3][4] + sh1[0][1] * sh2[1][4]),
                ksqrt03_10 * (sh1[1][2] * sh2[2][4] - sh1[1][0] * sh2[2][0])
                    - ksqrt01_10
                        * (sh1[2][2] * sh2[3][4] - sh1[2][0] * sh2[3][0]
                            + (sh1[0][2] * sh2[1][4] - sh1[0][0] * sh2[1][0])),
            ],
            [
                ksqrt04_15 * (sh1[1][2] * sh2[3][0] + sh1[1][0] * sh2[3][4])
                    + ksqrt01_05 * (sh1[2][2] * sh2[2][0] + sh1[2][0] * sh2[2][4])
                    - ksqrt01_60
                        * (sh1[2][2] * sh2[4][0]
                            + sh1[2][0] * sh2[4][4]
                            + (sh1[0][2] * sh2[0][0] + sh1[0][0] * sh2[0][4])),
                ksqrt08_05 * sh1[1][1] * sh2[3][0] + ksqrt06_05 * sh1[2][1] * sh2[2][0]
                    - ksqrt01_10 * (sh1[2][1] * sh2[4][0] + sh1[0][1] * sh2[0][0]),
                sh1[1][1] * sh2[3][1] + ksqrt03_04 * sh1[2][1] * sh2[2][1]
                    - ksqrt01_16 * (sh1[2][1] * sh2[4][1] + sh1[0][1] * sh2[0][1]),
                ksqrt08_09 * sh1[1][1] * sh2[3][2] + ksqrt02_03 * sh1[2][1] * sh2[2][2]
                    - ksqrt01_18 * (sh1[2][1] * sh2[4][2] + sh1[0][1] * sh2[0][2]),
                sh1[1][1] * sh2[3][3] + ksqrt03_04 * sh1[2][1] * sh2[2][3]
                    - ksqrt01_16 * (sh1[2][1] * sh2[4][3] + sh1[0][1] * sh2[0][3]),
                ksqrt08_05 * sh1[1][1] * sh2[3][4] + ksqrt06_05 * sh1[2][1] * sh2[2][4]
                    - ksqrt01_10 * (sh1[2][1] * sh2[4][4] + sh1[0][1] * sh2[0][4]),
                ksqrt04_15 * (sh1[1][2] * sh2[3][4] - sh1[1][0] * sh2[3][0])
                    + ksqrt01_05 * (sh1[2][2] * sh2[2][4] - sh1[2][0] * sh2[2][0])
                    - ksqrt01_60
                        * (sh1[2][2] * sh2[4][4] - sh1[2][0] * sh2[4][0]
                            + (sh1[0][2] * sh2[0][4] - sh1[0][0] * sh2[0][0])),
            ],
            [
                ksqrt01_06 * (sh1[1][2] * sh2[4][0] + sh1[1][0] * sh2[4][4])
                    + ksqrt01_06
                        * (sh1[2][2] * sh2[3][0] + sh1[2][0] * sh2[3][4]
                            - (sh1[0][2] * sh2[1][0] + sh1[0][0] * sh2[1][4])),
                sh1[1][1] * sh2[4][0] + (sh1[2][1] * sh2[3][0] - sh1[0][1] * sh2[1][0]),
                ksqrt05_08 * sh1[1][1] * sh2[4][1]
                    + ksqrt05_08 * (sh1[2][1] * sh2[3][1] - sh1[0][1] * sh2[1][1]),
                ksqrt05_09 * sh1[1][1] * sh2[4][2]
                    + ksqrt05_09 * (sh1[2][1] * sh2[3][2] - sh1[0][1] * sh2[1][2]),
                ksqrt05_08 * sh1[1][1] * sh2[4][3]
                    + ksqrt05_08 * (sh1[2][1] * sh2[3][3] - sh1[0][1] * sh2[1][3]),
                sh1[1][1] * sh2[4][4] + (sh1[2][1] * sh2[3][4] - sh1[0][1] * sh2[1][4]),
                ksqrt01_06 * (sh1[1][2] * sh2[4][4] - sh1[1][0] * sh2[4][0])
                    + ksqrt01_06
                        * (sh1[2][2] * sh2[3][4]
                            - sh1[2][0] * sh2[3][0]
                            - (sh1[0][2] * sh2[1][4] - sh1[0][0] * sh2[1][0])),
            ],
            [
                ksqrt01_04
                    * (sh1[2][2] * sh2[4][0] + sh1[2][0] * sh2[4][4]
                        - (sh1[0][2] * sh2[0][0] + sh1[0][0] * sh2[0][4])),
                ksqrt03_02 * (sh1[2][1] * sh2[4][0] - sh1[0][1] * sh2[0][0]),
                ksqrt15_16 * (sh1[2][1] * sh2[4][1] - sh1[0][1] * sh2[0][1]),
                ksqrt05_06 * (sh1[2][1] * sh2[4][2] - sh1[0][1] * sh2[0][2]),
                ksqrt15_16 * (sh1[2][1] * sh2[4][3] - sh1[0][1] * sh2[0][3]),
                ksqrt03_02 * (sh1[2][1] * sh2[4][4] - sh1[0][1] * sh2[0][4]),
                ksqrt01_04
                    * (sh1[2][2] * sh2[4][4]
                        - sh1[2][0] * sh2[4][0]
                        - (sh1[0][2] * sh2[0][4] - sh1[0][0] * sh2[0][0])),
            ],
        ];
        let mut result = [[0.0; 15]; 15];
        for (row, values) in sh1.iter().enumerate() {
            result[row][0..3].copy_from_slice(values);
        }
        for (row, values) in sh2.iter().enumerate() {
            result[3 + row][3..8].copy_from_slice(values);
        }
        for (row, values) in sh3.iter().enumerate() {
            result[8 + row][8..15].copy_from_slice(values);
        }
        Self(result)
    }
    pub(super) fn apply(&self, source: &[Vec3; 15]) -> [Vec3; 15] {
        self.0.map(|row| {
            row.iter()
                .zip(source)
                .map(|(weight, value)| *weight * *value)
                .sum()
        })
    }
}
