use egui::{Button, Ui};
use little_clip_core::engineering::logic::Lane;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(in crate::file_manager::logic_view) enum Role {
    Channel,
    Clock,
    Data,
    WordSelect,
    Mosi,
    Miso,
    ChipSelect,
    Signal,
}

impl Role {
    fn label(self) -> &'static str {
        match self {
            Self::Channel => "Channel",
            Self::Clock => "Clock",
            Self::Data => "Data",
            Self::WordSelect => "Word select",
            Self::Mosi => "MOSI",
            Self::Miso => "MISO",
            Self::ChipSelect => "Chip select",
            Self::Signal => "Signal",
        }
    }
}

pub(in crate::file_manager::logic_view) struct Pin<'a> {
    role: Role,
    channel: Channel<'a>,
}

enum Channel<'a> {
    Required(&'a mut usize),
    Optional(&'a mut Option<usize>),
}

impl<'a> Pin<'a> {
    pub fn required(role: Role, channel: &'a mut usize) -> Self {
        Self {
            role,
            channel: Channel::Required(channel),
        }
    }

    pub fn optional(role: Role, channel: &'a mut Option<usize>) -> Self {
        Self {
            role,
            channel: Channel::Optional(channel),
        }
    }

    fn lane(&self) -> Option<usize> {
        match &self.channel {
            Channel::Required(index) => Some(**index),
            Channel::Optional(index) => **index,
        }
    }

    fn assign(&mut self, lane: Option<usize>) {
        match (&mut self.channel, lane) {
            (Channel::Required(index), Some(lane)) => **index = lane,
            (Channel::Optional(index), lane) => **index = lane,
            (Channel::Required(_), None) => {}
        }
    }

    pub fn remap(&mut self, mapping: &[Option<usize>]) {
        if let Some(old) = self.lane() {
            self.assign(Some(
                mapping.get(old).copied().flatten().unwrap_or(usize::MAX),
            ));
        }
    }

    fn name<'b>(&self, lanes: &'b [Lane]) -> &'b str {
        self.lane().map_or("Unassigned", |index| {
            lanes
                .get(index)
                .map_or("Unavailable", |lane| lane.name.as_str())
        })
    }
}

#[derive(Default)]
pub(in crate::file_manager::logic_view) struct Wiring {
    active: Option<Role>,
    filter: String,
    focus_filter: bool,
}

impl Wiring {
    pub fn active(&self) -> bool {
        self.active.is_some()
    }

    pub fn close(&mut self) {
        self.active = None;
        self.filter.clear();
        self.focus_filter = false;
    }

    pub fn choose(&mut self, pins: &mut [Pin<'_>], lane: usize) {
        if let Some(pin) = pins.iter_mut().find(|pin| Some(pin.role) == self.active) {
            pin.assign(Some(lane));
            self.close();
        }
    }

    pub fn button(&mut self, ui: &mut Ui, pin: &Pin<'_>, lanes: &[Lane]) {
        let name = pin.name(lanes);
        let label = format!("{} · {name}", pin.role.label());
        let active = self.active == Some(pin.role);
        if ui
            .add(Button::new(label).selected(active).truncate())
            .on_hover_text(name)
            .clicked()
        {
            self.active = (!active).then_some(pin.role);
            self.filter.clear();
            self.focus_filter = !active;
        }
    }

    pub fn choices(&mut self, ui: &mut Ui, pins: &mut [Pin<'_>], lanes: &[Lane]) {
        let Some(pin) = pins.iter_mut().find(|pin| Some(pin.role) == self.active) else {
            return;
        };
        let focus = std::mem::take(&mut self.focus_filter);
        let Some(action) = picker(ui, pin, lanes, &mut self.filter, focus) else {
            return;
        };
        if let PickerAction::Assign(lane) = action {
            pin.assign(lane);
        }
        self.close();
    }

    pub fn ui(&mut self, ui: &mut Ui, pins: &mut [Pin<'_>], lanes: &[Lane]) {
        ui.horizontal_wrapped(|ui| {
            for pin in &*pins {
                self.button(ui, pin, lanes);
            }
        });
        self.choices(ui, pins, lanes);
    }
}

enum PickerAction {
    Assign(Option<usize>),
    Cancel,
}

fn picker(
    ui: &mut Ui,
    pin: &Pin<'_>,
    lanes: &[Lane],
    filter: &mut String,
    opening: bool,
) -> Option<PickerAction> {
    const COMPACT_LANES: usize = 8;
    ui.group(|ui| {
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            if pin.role == Role::Channel {
                ui.strong("Choose active channel");
            } else {
                ui.strong(format!("Assign {}", pin.role.label()));
            }
            if lanes.len() > COMPACT_LANES {
                let field = ui.add(
                    egui::TextEdit::singleline(filter)
                        .id_salt(("filter-wiring-lanes", pin.role))
                        .hint_text("Find channel")
                        .char_limit(64)
                        .desired_width(150.0),
                );
                if opening {
                    field.request_focus();
                }
            }
            if matches!(&pin.channel, Channel::Optional(_))
                && ui
                    .add(Button::new("Unassigned").selected(pin.lane().is_none()))
                    .clicked()
            {
                action = Some(PickerAction::Assign(None));
            }
            if ui.button("Cancel").clicked() {
                action = Some(PickerAction::Cancel);
            }
        });
        let query = filter.trim().to_lowercase();
        let matching: Vec<_> = lanes
            .iter()
            .enumerate()
            .filter(|(_, lane)| query.is_empty() || lane.name.to_lowercase().contains(&query))
            .collect();
        let mut choice = |ui: &mut Ui, (index, lane): (usize, &Lane)| {
            ui.push_id(index, |ui| {
                if ui
                    .add(
                        Button::new(&lane.name)
                            .selected(pin.lane() == Some(index))
                            .truncate(),
                    )
                    .on_hover_text(&lane.name)
                    .clicked()
                {
                    action = Some(PickerAction::Assign(Some(index)));
                }
            });
        };
        if lanes.len() <= COMPACT_LANES {
            ui.horizontal_wrapped(|ui| {
                for &lane in &matching {
                    choice(ui, lane);
                }
            });
        } else {
            let row_height = ui.spacing().interact_size.y.max(24.0);
            egui::ScrollArea::vertical()
                .id_salt(("wiring-lanes", pin.role, &query))
                .max_height(106.0)
                .auto_shrink([false, true])
                .show_rows(ui, row_height, matching.len(), |ui, rows| {
                    for &lane in &matching[rows] {
                        choice(ui, lane);
                    }
                });
        }
        if matching.is_empty() {
            ui.weak("No matching channels");
        }
        action
    })
    .inner
}
