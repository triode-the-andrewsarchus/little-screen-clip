use super::{Star, dot, sky::catalog::Catalog, unit};
use egui::{Pos2, Rect, Vec2};

#[derive(Clone, Copy)]
pub(in crate::file_manager::geography) struct View {
    pub axes: [[f32; 3]; 3],
    pub focal: f32,
}

impl View {
    pub(super) fn ray(self, offset: Vec2) -> [f32; 3] {
        let [east, north, outward] = self.axes;
        let slope = offset / self.focal;
        unit(std::array::from_fn(|i| {
            east[i] * slope.x - north[i] * slope.y - outward[i]
        }))
    }

    pub(super) fn offset(self, direction: [f32; 3]) -> Option<Vec2> {
        let [x, y, z] = self.axes.map(|axis| dot(axis, direction));
        (z < 0.0).then(|| egui::vec2(x, -y) * (self.focal / -z))
    }

    pub(in crate::file_manager::geography) fn pick(
        self,
        catalog: &Catalog,
        rect: Rect,
        pointer: Pos2,
        exposed: impl Fn(Pos2) -> bool,
    ) -> Option<&Star> {
        if self.focal < 32.0 {
            return None;
        }
        let center = rect.center();
        let ray = self.ray(pointer - center);
        const PICK_RADIUS: f32 = 5.0;
        catalog
            .nearby(ray, PICK_RADIUS / self.focal)
            .filter_map(|star| {
                let point = center + self.offset(star.direction)?;
                let distance = pointer.distance_sq(point);
                (distance <= PICK_RADIUS.powi(2) && rect.contains(point) && exposed(point))
                    .then_some((star, distance))
            })
            .min_by(|(a, da), (b, db)| {
                a.magnitude
                    .total_cmp(&b.magnitude)
                    .then_with(|| da.total_cmp(db))
            })
            .map(|(star, _)| star)
    }
}

pub(in crate::file_manager::geography) fn show_name(ui: &mut egui::Ui, star: &Star) {
    ui.label(star.name.as_ref());
    if let Some(hip) = star.hip
        && !star.name.starts_with("HIP ")
    {
        ui.label(format!("HIP {hip}"));
    }
}
