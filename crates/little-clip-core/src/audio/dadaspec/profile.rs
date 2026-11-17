//! Opt-in native CUDA experiment. Never loaded by either desktop application.
use super::*;
use std::{
    ffi::{CStr, CString, c_void},
    time::Instant,
};

struct Pcm {
    left: Vec<f32>,
    right: Vec<f32>,
}

fn decode(path: &Path) -> Pcm {
    use symphonia::core::{audio::SampleBuffer, codecs::DecoderOptions, errors::Error};
    let cancel = AtomicBool::new(false);
    let file = crate::kernel_interface::open_content(path, true).unwrap();
    let mut format =
        crate::audio::source::format_reader(file, path, &cancel, symphonia::default::get_probe())
            .unwrap();
    let track = format.default_track().unwrap().clone();
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .unwrap();
    let frames = track.codec_params.n_frames.unwrap() as usize;
    let mut pcm = Pcm {
        left: Vec::with_capacity(frames),
        right: Vec::with_capacity(frames),
    };
    let mut converted: Option<SampleBuffer<f32>> = None;
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(Error::IoError(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(error) => panic!("{error}"),
        };
        if packet.track_id() != track.id {
            continue;
        }
        let buffer = decoder.decode(&packet).unwrap();
        assert_eq!(buffer.spec().channels.count(), 2);
        let frames = buffer.frames();
        let samples = crate::audio::decode::conversion_buffer(
            &mut converted,
            buffer.capacity(),
            *buffer.spec(),
        );
        samples.copy_planar_ref(buffer);
        pcm.left.extend_from_slice(&samples.samples()[..frames]);
        pcm.right
            .extend_from_slice(&samples.samples()[frames..frames * 2]);
    }
    pcm
}

type Create = unsafe extern "C" fn(i32, i32, i32, i32, *mut f64, *mut f64) -> *mut c_void;
type Run = unsafe extern "C" fn(*mut c_void, *const *const f32, *mut f32) -> i32;
type Columns = unsafe extern "C" fn(*mut c_void) -> i32;
type Destroy = unsafe extern "C" fn(*mut c_void);
struct Cuda {
    library: *mut c_void,
    handle: *mut c_void,
    run: Run,
    destroy: Destroy,
    columns: usize,
    frames: usize,
    files: usize,
}
impl Cuda {
    fn new(frames: usize, files: usize, width: u32) -> (Self, f64, f64) {
        let path = CString::new(
            std::env::var("DADASPEC_CUDA_PROBE")
                .expect("set DADASPEC_CUDA_PROBE to the owned, compiled probe library"),
        )
        .unwrap();
        // SAFETY: This ignored development profile loads the explicitly supplied
        // local probe with the matching checked ABI in scripts/dadaspec_cuda_probe.cu.
        // No library or foreign pointer is accepted by the application renderer.
        unsafe {
            let library = libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
            assert!(
                !library.is_null(),
                "{}",
                CStr::from_ptr(libc::dlerror()).to_string_lossy()
            );
            let create: Create = symbol(library, c"dadaspec_probe_create");
            let run: Run = symbol(library, c"dadaspec_probe_run");
            let destroy: Destroy = symbol(library, c"dadaspec_probe_destroy");
            let columns: Columns = symbol(library, c"dadaspec_probe_columns");
            let (mut context_ms, mut setup_ms) = (0.0, 0.0);
            let handle = create(
                frames.try_into().unwrap(),
                44100,
                width as i32,
                files as i32,
                &mut context_ms,
                &mut setup_ms,
            );
            assert!(!handle.is_null(), "native CUDA setup failed");
            (
                Self {
                    library,
                    handle,
                    run,
                    destroy,
                    columns: columns(handle) as usize,
                    frames,
                    files,
                },
                context_ms,
                setup_ms,
            )
        }
    }
    fn render(&mut self, pcm: &[Pcm], width: u32) -> (Vec<ImageThumbnail>, f64, f64) {
        assert_eq!(pcm.len(), self.files);
        assert!(
            pcm.iter()
                .all(|p| p.left.len() == self.frames && p.right.len() == self.frames)
        );
        let started = Instant::now();
        let channels: Vec<_> = pcm
            .iter()
            .flat_map(|p| [p.left.as_ptr(), p.right.as_ptr()])
            .collect();
        let mut powers = vec![[[0.0; MELS]; 2]; self.files * self.columns];
        // SAFETY: All borrowed PCM channels and the contiguous output array
        // match the dimensions captured by this owned context. run synchronizes
        // CUDA before returning, so none of these borrows outlive this call.
        assert_eq!(
            unsafe { (self.run)(self.handle, channels.as_ptr(), powers.as_mut_ptr().cast()) },
            0
        );
        let compute_ms = started.elapsed().as_secs_f64() * 1000.0;
        let started = Instant::now();
        let images = std::thread::scope(|scope| {
            let jobs: Vec<_> = powers
                .chunks_exact(self.columns)
                .map(|powers| {
                    scope.spawn(move || {
                        let mut analyzer = Analyzer::new();
                        analyzer.powers = powers.to_vec();
                        analyzer.finish(width, 60, &AtomicBool::new(false)).unwrap()
                    })
                })
                .collect();
            jobs.into_iter().map(|job| job.join().unwrap()).collect()
        });
        (images, compute_ms, started.elapsed().as_secs_f64() * 1000.0)
    }
}
unsafe fn symbol<T: Copy>(library: *mut c_void, name: &CStr) -> T {
    // SAFETY: Callers select fixed names and ABI signatures from the owned probe.
    let pointer = unsafe { libc::dlsym(library, name.as_ptr()) };
    assert!(!pointer.is_null());
    assert_eq!(std::mem::size_of::<T>(), std::mem::size_of_val(&pointer));
    unsafe { std::mem::transmute_copy(&pointer) }
}
impl Drop for Cuda {
    fn drop(&mut self) {
        // SAFETY: Destruction synchronously releases this context's owned GPU
        // allocations before unloading the library that supplies its functions.
        unsafe {
            (self.destroy)(self.handle);
            libc::dlclose(self.library);
        }
    }
}

fn batch<T: Send>(paths: &[std::path::PathBuf], work: impl Fn(&Path) -> T + Sync) -> Vec<T> {
    std::thread::scope(|scope| {
        let jobs: Vec<_> = paths
            .iter()
            .map(|path| scope.spawn(|| work(path)))
            .collect();
        jobs.into_iter().map(|job| job.join().unwrap()).collect()
    })
}
fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

#[test]
#[ignore = "requires explicit native CUDA probe library; profiles three owned WAV fixtures"]
fn native_cuda_three_file_budget() {
    let seconds: u32 = std::env::var("DADASPEC_PROFILE_SECONDS")
        .unwrap_or_else(|_| "60".into())
        .parse()
        .unwrap();
    assert!((1..=300).contains(&seconds));
    let width = 1024;
    let fixtures: Vec<_> = (0..3)
        .map(|file| {
            crate::audio::test_support::generate_fixture_wav(
                &format!("gpu-budget-{file}"),
                seconds as f32,
            )
        })
        .collect();
    let paths: Vec<_> = fixtures.iter().map(|(_, path)| path.clone()).collect();
    for dadaspec in [false, true] {
        let mut times = Vec::new();
        for run in 0..10 {
            let started = Instant::now();
            batch(&paths, |path| {
                let file = crate::kernel_interface::open_content(path, true).unwrap();
                let cancel = AtomicBool::new(false);
                if dadaspec {
                    std::hint::black_box(
                        render_file(file, path, 0.0, seconds as f64, width, 60, &cancel).unwrap(),
                    );
                } else {
                    std::hint::black_box(
                        crate::audio::compute_preview_waveform(
                            file,
                            path,
                            seconds as f32,
                            512,
                            None,
                            &cancel,
                        )
                        .unwrap(),
                    );
                }
            });
            if run > 0 {
                times.push(started.elapsed().as_secs_f64() * 1000.0);
            }
        }
        let median = median(&mut times);
        println!(
            "{}",
            serde_json::json!({"case":if dadaspec {"cpu_dadaspec_three"} else {"cpu_waveform_three"}, "seconds":seconds,"width":width,"median_ms":median,"all_ms":times})
        );
    }
    let started = Instant::now();
    let pcm = batch(&paths, decode);
    let decode_ms = started.elapsed().as_secs_f64() * 1000.0;
    let (mut gpu, context_ms, setup_ms) = Cuda::new(pcm[0].left.len(), paths.len(), width);
    let (images, compute_ms, compose_ms) = gpu.render(&pcm, width);
    let cold_ms = started.elapsed().as_secs_f64() * 1000.0;
    println!(
        "{}",
        serde_json::json!({"case":"gpu_dadaspec_three_cold","seconds":seconds,"width":width,"total_ms":cold_ms,"decode_ms":decode_ms,"context_ms":context_ms,"setup_ms":setup_ms,"compute_ms":compute_ms,"compose_ms":compose_ms})
    );
    for (path, image) in paths.iter().zip(&images) {
        let reference = render_file(
            crate::kernel_interface::open_content(path, true).unwrap(),
            path,
            0.0,
            seconds as f64,
            width,
            60,
            &AtomicBool::new(false),
        )
        .unwrap();
        let errors: Vec<_> = reference
            .premultiplied_rgba
            .iter()
            .zip(&image.premultiplied_rgba)
            .map(|(a, b)| a.abs_diff(*b))
            .collect();
        let mean = errors.iter().map(|&e| e as f64).sum::<f64>() / errors.len() as f64;
        assert!(
            mean < 0.5 && *errors.iter().max().unwrap() <= 4,
            "GPU pixel mismatch: mean {mean}, max {:?}",
            errors.iter().max()
        );
    }
    // Also measure a best-case warm GPU after clock/power-state ramp-up, not
    // just a cached context/FFT plan. Never change system GPU clock settings.
    let conditioning = Instant::now();
    while conditioning.elapsed() < std::time::Duration::from_secs(1) {
        std::hint::black_box(gpu.render(&pcm, width));
    }
    let (mut total, mut decoding, mut compute, mut composition) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for _ in 0..9 {
        let started = Instant::now();
        let pcm = batch(&paths, decode);
        let decode_ms = started.elapsed().as_secs_f64() * 1000.0;
        let (images, compute_ms, compose_ms) = gpu.render(&pcm, width);
        std::hint::black_box(images);
        total.push(started.elapsed().as_secs_f64() * 1000.0);
        decoding.push(decode_ms);
        compute.push(compute_ms);
        composition.push(compose_ms);
    }
    let result = serde_json::json!({"case":"gpu_dadaspec_three_warm","seconds":seconds,"width":width,"median_ms":median(&mut total),"decode_ms":median(&mut decoding),"compute_ms":median(&mut compute),"compose_ms":median(&mut composition),"all_ms":total});
    println!("{result}");
}
