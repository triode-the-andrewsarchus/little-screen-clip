use crate::ScreenRenderer;
use little_clip_core::engineering::radio::{BANDS, Summary, frequency_label};
use tiny_skia::Pixmap;

impl ScreenRenderer {
    pub(super) fn draw_radio_channels(
        &self,
        pixmap: &mut Pixmap,
        radio: &Summary,
        [x, y, width, height]: [f32; 4],
    ) {
        let left = x + 14.0;
        let right = x + width - 14.0;
        let top = y + 150.0;
        let height = (height - 170.0).max(28.0);
        let heading = radio.model.as_deref().unwrap_or(radio.format.title());
        let totals = if radio.count > 0 {
            let bands = BANDS
                .into_iter()
                .zip(radio.band_counts)
                .filter(|(_, count)| *count > 0)
                .map(|(name, count)| format!("{name} {count}"))
                .collect::<Vec<_>>()
                .join(" · ");
            format!("{} channels · {bands}", radio.count)
        } else {
            format!("{} memory records", radio.records)
        };
        let ink = self.color_from_hex(&self.palette.colors.text_primary);
        let muted = self.color_from_hex(&self.palette.colors.text_secondary);
        self.draw_text_clipped(pixmap, heading, left, y + 119.0, 15.0, ink, false, right);
        self.draw_text_clipped(pixmap, &totals, left, y + 140.0, 13.0, muted, false, right);
        if self
            .draw_card_viewport_frame(pixmap, left, top, right - left, height)
            .is_none()
        {
            return;
        }
        let split = left + (right - left) * 0.51;
        let rows = ((height - 8.0) / 23.0).clamp(1.0, 5.0) as usize;
        for (index, channel) in radio.channels.iter().take(rows).enumerate() {
            let top = top + 5.0 + 23.0 * index as f32;
            let title = format!("{}  {}", channel.number, channel.name);
            self.draw_text_clipped(
                pixmap,
                &title,
                left + 5.0,
                top,
                13.0,
                ink,
                true,
                split - 3.0,
            );
            let summary = format!("{}  {}", frequency_label(channel.receive_hz), channel.mode);
            self.draw_text_clipped(
                pixmap,
                &summary,
                split,
                top,
                13.0,
                muted,
                false,
                right - 4.0,
            );
        }
        if radio.channels.is_empty()
            && let Some([first, last]) = radio.address_span
        {
            self.draw_text_clipped(
                pixmap,
                &format!("0x{first:X} – 0x{last:X}"),
                left + 5.0,
                top + 6.0,
                13.0,
                ink,
                true,
                right - 5.0,
            );
        }
    }
}
