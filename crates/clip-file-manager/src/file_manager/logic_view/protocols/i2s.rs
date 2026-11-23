use super::{DECODE_EDGES, Decoded, Inputs, TraceInput, lane_window, named_lane};
use egui::Ui;
use little_clip_core::engineering::{
    logic::Lane,
    logic_decode::{I2sConfig, I2sFormat, i2s},
};
use std::{ops::Range, sync::atomic::AtomicBool};

#[derive(Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub(super) struct I2sInputs {
    pub channels: [usize; 3],
    config: I2sConfig,
}

impl I2sInputs {
    pub(super) fn new(lanes: &[Lane]) -> Self {
        let mut picked = [
            named_lane(lanes, &["BCLK", "BCK", "SCK"]),
            named_lane(lanes, &["WS", "LRCLK", "LRCK", "FS"]),
            named_lane(lanes, &["SD", "SDATA", "DOUT", "DIN", "DATA"]),
        ];
        for index in 0..picked.len() {
            if picked[index].is_none() {
                picked[index] = (0..lanes.len()).find(|lane| !picked.contains(&Some(*lane)));
            }
        }
        Self {
            channels: picked.map(|lane| lane.unwrap_or(0)),
            config: I2sConfig::default(),
        }
    }

    pub(super) fn controls(&mut self, ui: &mut Ui) {
        for format in [I2sFormat::Philips, I2sFormat::LeftJustified] {
            if ui
                .add(egui::Button::new(format.label()).selected(self.config.format == format))
                .clicked()
                && self.config.format != format
            {
                self.config.format = format;
                self.config.left_high = format == I2sFormat::LeftJustified;
            }
        }
        ui.add(
            egui::DragValue::new(&mut self.config.bits)
                .range(8..=32)
                .suffix(" bits"),
        );
        ui.toggle_value(&mut self.config.left_high, "WS high = left");
    }

    pub(super) fn validate(self) -> Result<Inputs, &'static str> {
        let [clock, ws, data] = self.channels;
        if clock == ws || clock == data || ws == data {
            return Err("Choose different clock, word select and data channels.");
        }
        Ok(Inputs::I2s(self))
    }

    pub(super) fn read(
        self,
        capture: &dyn TraceInput,
        span: Range<f64>,
        cancel: &AtomicBool,
    ) -> Result<Decoded, String> {
        let window = self
            .channels
            .into_iter()
            .try_fold(span, |span, lane| lane_window(capture, lane, span))?;
        let [clock, ws, data] = self.channels;
        let read = |lane| capture.read_edges(lane, window.clone(), DECODE_EDGES, cancel);
        let clock = read(clock)?;
        let ws = read(ws)?;
        let data = read(data)?;
        let inspected_until = clock
            .inspected_until
            .min(ws.inspected_until)
            .min(data.inspected_until);
        let events = i2s(&clock, &ws, &data, self.config)?;
        Ok(Decoded {
            window,
            inspected_until,
            events,
        })
    }
}
