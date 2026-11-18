//! Opt-in native renderer checks, isolated from the desktop/compositor.
use super::*;
use std::ffi::{CString, c_char, c_void};
type Display = *mut c_void;
#[link(name = "EGL")]
unsafe extern "C" {
    fn eglGetPlatformDisplay(
        platform: u32,
        native: *mut c_void,
        attributes: *const isize,
    ) -> Display;
    fn eglInitialize(display: Display, major: *mut i32, minor: *mut i32) -> u32;
    fn eglBindAPI(api: u32) -> u32;
    fn eglChooseConfig(
        display: Display,
        attributes: *const i32,
        configs: *mut Display,
        size: i32,
        count: *mut i32,
    ) -> u32;
    fn eglCreatePbufferSurface(
        display: Display,
        config: Display,
        attributes: *const i32,
    ) -> Display;
    fn eglCreateContext(
        display: Display,
        config: Display,
        shared: Display,
        attributes: *const i32,
    ) -> Display;
    fn eglMakeCurrent(display: Display, draw: Display, read: Display, context: Display) -> u32;
    fn eglGetProcAddress(name: *const c_char) -> *const c_void;
    fn eglDestroyContext(display: Display, context: Display) -> u32;
    fn eglDestroySurface(display: Display, surface: Display) -> u32;
    fn eglTerminate(display: Display) -> u32;
}
struct Headless {
    display: Display,
    surface: Display,
    context: Display,
}
impl Headless {
    fn new() -> Self {
        // SAFETY: EGL creates a private pbuffer and context on the calling test
        // thread. All attribute arrays are terminated and outputs initialized.
        unsafe {
            let display = eglGetPlatformDisplay(0x31dd, std::ptr::null_mut(), std::ptr::null());
            assert!(!display.is_null(), "No surfaceless EGL display");
            let (mut major, mut minor) = (0, 0);
            assert_ne!(eglInitialize(display, &mut major, &mut minor), 0);
            assert_ne!(eglBindAPI(0x30a2), 0);
            let attributes = [
                0x3033, 1, 0x3040, 8, 0x3024, 8, 0x3023, 8, 0x3022, 8, 0x3021, 8, 0x3038,
            ];
            let (mut config, mut count) = (std::ptr::null_mut(), 0);
            assert_ne!(
                eglChooseConfig(display, attributes.as_ptr(), &mut config, 1, &mut count),
                0
            );
            assert!(count > 0);
            let surface = eglCreatePbufferSurface(
                display,
                config,
                [0x3057, 512, 0x3056, 384, 0x3038].as_ptr(),
            );
            let context = eglCreateContext(
                display,
                config,
                std::ptr::null_mut(),
                [0x3098, 3, 0x30fb, 3, 0x30fd, 1, 0x3038].as_ptr(),
            );
            assert!(!surface.is_null() && !context.is_null());
            assert_ne!(eglMakeCurrent(display, surface, surface, context), 0);
            Self {
                display,
                surface,
                context,
            }
        }
    }
    fn gl(&self) -> glow::Context {
        // SAFETY: this thread retains the matching context as current until Drop.
        unsafe {
            glow::Context::from_loader_function(|name| {
                eglGetProcAddress(CString::new(name).unwrap().as_ptr())
            })
        }
    }
}
impl Drop for Headless {
    fn drop(&mut self) {
        // SAFETY: all GPU owners are destroyed before this private test context.
        unsafe {
            eglMakeCurrent(
                self.display,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            eglDestroyContext(self.display, self.context);
            eglDestroySurface(self.display, self.surface);
            eglTerminate(self.display);
        }
    }
}

#[test]
#[ignore = "requires surfaceless EGL OpenGL 3.3; run explicitly on hardware or Mesa llvmpipe"]
fn native_renderer_reuses_tiles_on_camera_changes_and_retires_resources() {
    let context = Headless::new();
    let gl = context.gl();
    let count = PADDED_EDGE * PADDED_EDGE;
    let tile = Arc::new(SourceTile {
        id: TileId {
            level: 0,
            x: 1,
            y: 0,
        },
        color: [80, 160, 60, 255].repeat(count),
        elevation: [1000.0, 1.0].repeat(count),
        water: vec![0; count],
        elevation_range: Some([1000.0, 1000.0]),
    });
    let mut renderer = Renderer::default();
    let mut frame = Frame {
        source: 1,
        size: [512., 384.],
        camera: GlobeCamera {
            center_lat: 0.,
            center_lon: 45.,
            ..GlobeCamera::default()
        },
        style: BasemapStyle::Natural,
        contours: None,
        range: [1000., 1000.],
        tiles: vec![tile],
        scene: None,
        layers: Layers {
            countries: true,
            states: true,
            rivers: true,
            roads: true,
        },
    };
    let info = egui::PaintCallbackInfo {
        viewport: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(512., 384.)),
        clip_rect: egui::Rect::EVERYTHING,
        pixels_per_point: 1.,
        screen_size_px: [512, 384],
    };
    // SAFETY: context remains current on this thread through all draws/teardown.
    unsafe {
        assert!(!renderer.paint(&gl, &info, &frame).unwrap());
        assert_eq!(renderer.tiles.len(), 1);
        let textures = renderer.tiles.values().next().unwrap().textures;
        let bytes = renderer.bytes;
        let mut pixels = vec![0u8; 512 * 384 * 4];
        gl.read_pixels(
            0,
            0,
            512,
            384,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(&mut pixels),
        );
        assert!(
            pixels
                .chunks_exact(4)
                .any(|p| p[1] as u16 > p[0] as u16 + 20 && p[1] as u16 > p[2] as u16 + 20),
            "Native callback did not draw the source tile"
        );
        for zoom in [1.0, 3.15, 6.0, 512.0] {
            frame.camera.zoom = zoom;
            frame.camera.center_lon += 0.0049;
            assert!(!renderer.paint(&gl, &info, &frame).unwrap());
            assert_eq!(renderer.tiles.values().next().unwrap().textures, textures);
            assert_eq!(renderer.bytes, bytes);
        }
        frame.source = 2;
        renderer.paint(&gl, &info, &frame).unwrap();
        assert_eq!(
            renderer.tiles.len(),
            2,
            "Asset generations must use separate texture identities"
        );
        assert_ne!(
            renderer
                .tiles
                .get(&(2, frame.tiles[0].id))
                .unwrap()
                .textures,
            textures
        );
        renderer.destroy(&gl);
        assert!(
            renderer.tiles.is_empty()
                && renderer.vectors.is_empty()
                && renderer.resources.is_none()
        );
        assert_eq!(gl.get_error(), glow::NO_ERROR);
    }
}
