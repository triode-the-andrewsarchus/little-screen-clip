use super::{Body, Id, assets};
use crate::file_manager::geography::{map_camera, sky};
use little_clip_core::{
    geography::basemap::{GlobeCamera, perspective::Perspective},
    jobs::task::Task,
};
use std::{path::Path, time::Duration};

pub(in crate::file_manager::geography) struct World {
    pub body: &'static Body,
    pub camera: GlobeCamera,
    middle: map_camera::MiddleDrag,
    inspected: bool,
    job: Option<Task<Result<Option<assets::Assets>, String>>>,
    assets: Option<assets::Assets>,
    pub error: Option<String>,
}

impl World {
    pub fn new(body: &'static Body) -> Self {
        let mut camera = map_camera::lunar_camera();
        camera.center_lat = 21.0;
        camera.zoom = if bands(body.id).is_empty() { 1.0 } else { 0.50 };
        Self {
            body,
            camera,
            middle: Default::default(),
            inspected: false,
            job: None,
            assets: None,
            error: None,
        }
    }

    pub fn navigate(&mut self, response: &egui::Response) {
        map_camera::navigate(
            &mut self.camera,
            response,
            Some(radius_m(self.body)),
            &mut self.middle,
        );
    }

    pub fn reload(&mut self) {
        self.job = None;
        self.inspected = false;
        self.error = None;
    }

    pub fn poll(&mut self, ctx: &egui::Context, root: &Path) {
        if let Some(result) = self.job.as_ref().and_then(Task::poll) {
            self.job = None;
            self.inspected = true;
            match result {
                Ok(Ok(assets)) => self.assets = assets,
                Ok(Err(error)) => self.error = Some(error),
                Err(error) => self.error = Some(format!("World asset worker: {error}")),
            }
        }
        if !self.inspected && self.job.is_none() {
            let root = root.to_owned();
            let id = self.body.id;
            self.job = Some(Task::start(
                super::super::super::geography_downloader::asset_workers(),
                Duration::from_secs(30),
                move |_| assets::Assets::load(&root, id),
            ));
        }
        if self.job.is_some() {
            ctx.request_repaint_after(Duration::from_millis(80));
        }
    }

    pub fn pending(&self) -> bool {
        !self.inspected
    }

    pub fn installed(&self) -> bool {
        self.assets.is_some()
    }

    pub fn frame(&self, size: [f32; 2]) -> Frame {
        Frame::new(self.body, self.camera, size, self.texture())
    }

    pub fn texture(&self) -> Option<sky::Texture> {
        self.assets.as_ref()?.texture.clone()
    }

    pub fn ephemeris(&self) -> Option<&[[f64; 3]]> {
        self.assets.as_ref()?.ephemeris.as_deref()
    }
}

fn radius_m(body: &Body) -> f64 {
    body.radius.unwrap_or(5.0) * 1000.0
}

pub(in crate::file_manager::geography) struct Frame {
    pub pose: Perspective,
    pub zoom: f32,
    pub color: [f32; 3],
    pub texture: Option<sky::Texture>,
    pub bands: &'static [[f32; 3]],
    pub gas: bool,
}

impl Frame {
    pub fn new(
        body: &Body,
        camera: GlobeCamera,
        size: [f32; 2],
        texture: Option<sky::Texture>,
    ) -> Self {
        let color = body.color().to_array();
        Self {
            pose: Perspective::new(&camera, size, radius_m(body)),
            zoom: camera.zoom as f32,
            color: [color[0], color[1], color[2]].map(|value| value as f32 / 255.0),
            texture,
            bands: bands(body.id),
            gas: matches!(body.id, 599 | 699 | 799 | 899),
        }
    }
}

fn bands(id: Id) -> &'static [[f32; 3]] {
    // Inner and outer radii in km, with presentation opacity weights.
    // NASA PDS Ring-Moon Systems Node and USGS Gazetteer planetary ring tables.
    // Haumea's ring: Ortiz et al. (2017), https://doi.org/10.1038/nature24051.
    match id {
        599 => &[
            [92_000., 122_500., 0.09],
            [122_500., 129_000., 0.44],
            [129_000., 182_000., 0.11],
            [182_000., 226_000., 0.05],
        ],
        699 => &[
            [67_000., 74_510., 0.19],
            [74_658., 92_000., 0.43],
            [92_000., 117_580., 0.75],
            [122_170., 133_590., 0.57],
            [133_618., 136_775., 0.53],
            [140_180., 140_360., 0.42],
        ],
        799 => &[
            [41_820., 41_865., 0.27],
            [42_225., 42_245., 0.23],
            [44_710., 44_755., 0.33],
            [45_645., 45_680., 0.31],
            [47_160., 47_200., 0.43],
            [48_295., 48_345., 0.34],
            [50_990., 51_185., 0.73],
            [66_100., 69_900., 0.1],
        ],
        899 => &[
            [41_700., 42_220., 0.21],
            [53_150., 53_310., 0.34],
            [53_500., 57_200., 0.13],
            [61_900., 62_025., 0.5],
        ],
        2_136_108 => &[[2252., 2322., 0.48]],
        _ => &[],
    }
}
