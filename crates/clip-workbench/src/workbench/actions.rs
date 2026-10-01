use super::*;

impl WorkbenchApp {
    pub(crate) fn handle_mirror_action(&mut self, action: Option<MirrorAction>) {
        match action {
            Some(MirrorAction::Click(slot) | MirrorAction::DoubleClick(slot)) => {
                if slot != SlotId::Undo {
                    if let Some(index) = self
                        .selected_slots
                        .iter()
                        .position(|&selected| selected == slot)
                    {
                        if self.selected_slots.len() > 1 {
                            self.selected_slots.remove(index);
                        }
                    } else {
                        self.selected_slots.push(slot);
                    }
                    self.dirty_mirror = true;
                    self.selected_action_index = 0;
                }
            }
            Some(MirrorAction::Context(slot)) => {
                if let Some(item) = self.state.get(slot).cloned() {
                    self.open_slot_preview_modal(&item);
                }
            }
            Some(MirrorAction::DropSwap { from, to }) => {
                let _ = send_ipc_request(&IpcRequest::SwapSlots(from, to));
                self.state.swap_slots(from, to);
                self.dirty_mirror = true;
                self.status_msg = Some(format!("Swapped {} and {}", from.label(), to.label()));
            }
            None => {}
        }
    }

    pub(crate) fn open_diff_viewer(&mut self) {
        let (lhs_target, rhs_target) = if self.selected_slots.len() >= 2 {
            let items: Vec<&ClipItem> = self
                .selected_slots
                .iter()
                .filter_map(|s| self.state.get(*s))
                .collect();
            let img_items: Vec<&ClipItem> = items
                .iter()
                .filter(|i| {
                    resolve_diff_target(i)
                        .map(|t| t.is_image())
                        .unwrap_or(false)
                })
                .copied()
                .collect();
            if img_items.len() >= 2 {
                (
                    resolve_diff_target(img_items[0]),
                    resolve_diff_target(img_items[1]),
                )
            } else if let Some((t1, t2)) = items
                .iter()
                .find_map(|i| resolve_diff_pair(i).filter(|(a, b)| a.is_image() && b.is_image()))
            {
                (Some(t1), Some(t2))
            } else if items.len() >= 2 {
                (resolve_diff_target(items[0]), resolve_diff_target(items[1]))
            } else if items.len() == 1 {
                if let Some((t1, t2)) = resolve_diff_pair(items[0]) {
                    (Some(t1), Some(t2))
                } else {
                    (None, None)
                }
            } else {
                (None, None)
            }
        } else if self.selected_slots.len() == 1 {
            let selected_item = self.state.get(self.selected_slots[0]);
            if let Some(item) = selected_item {
                if let Some((t1, t2)) = resolve_diff_pair(item) {
                    (Some(t1), Some(t2))
                } else {
                    let other = if self.selected_slots[0] == SlotId::LeftHand {
                        self.state
                            .right_hand()
                            .or_else(|| self.state.get(SlotId::LeftPrior1))
                            .or_else(|| self.state.get(SlotId::Undo))
                    } else {
                        self.state.left_hand()
                    };
                    (
                        resolve_diff_target(item),
                        other.and_then(resolve_diff_target),
                    )
                }
            } else {
                (None, None)
            }
        } else {
            let left = self.state.left_hand();
            let right = self.state.right_hand();
            if let (Some(l), Some(r)) = (left, right) {
                if let Some((t1, t2)) =
                    resolve_diff_pair(l).filter(|(a, b)| a.is_image() && b.is_image())
                {
                    if !resolve_diff_target(r)
                        .map(|t| t.is_image())
                        .unwrap_or(false)
                    {
                        (Some(t1), Some(t2))
                    } else {
                        (resolve_diff_target(l), resolve_diff_target(r))
                    }
                } else {
                    (resolve_diff_target(l), resolve_diff_target(r))
                }
            } else if let Some(l) = left {
                if let Some((t1, t2)) = resolve_diff_pair(l) {
                    (Some(t1), Some(t2))
                } else {
                    let prior = self
                        .state
                        .get(SlotId::LeftPrior1)
                        .or_else(|| self.state.get(SlotId::Undo))
                        .or_else(|| self.state.get(SlotId::LeftPrior2));
                    (resolve_diff_target(l), prior.and_then(resolve_diff_target))
                }
            } else {
                (None, None)
            }
        };

        match (lhs_target, rhs_target) {
            (Some(lhs), Some(rhs)) => {
                if lhs.is_image() && !rhs.is_image() {
                    self.status_msg = Some(
                        "Comparison target is not an image (both targets must be images or text)"
                            .to_string(),
                    );
                } else if !lhs.is_image() && rhs.is_image() {
                    self.status_msg = Some(
                        "Left target is not an image (both targets must be images or text)"
                            .to_string(),
                    );
                } else {
                    self.replace_viewer(Some(ViewerSession::Diff(DiffViewerState::new(
                        lhs,
                        rhs,
                        DiffDisplayMode::SideBySide,
                    ))));
                }
            }
            (Some(_), None) => {
                self.status_msg = Some(
                    "Diff requires two items. Copy a second item to Right Hand or select 2 slots."
                        .to_string(),
                );
            }
            (None, _) => {
                self.status_msg = Some(
                    "No item selected or slot is empty. Push or copy content first.".to_string(),
                );
            }
        }
    }

    pub(crate) fn open_binary_diff_viewer(&mut self) {
        let selected_items = self.get_selected_items();
        if let Some(((l_lbl, l_bytes), (r_lbl, r_bytes))) =
            resolve_binary_diff_pair(&selected_items, &self.state)
        {
            self.replace_viewer(Some(ViewerSession::BinaryDiff(BinaryDiffViewState::new(
                l_lbl, l_bytes, r_lbl, r_bytes,
            ))));
        } else {
            self.status_msg =
                Some("Binary diff requires 2 slots or 1 slot with 2 files/paths.".to_string());
        }
    }

    pub(crate) fn get_selected_items(&self) -> Vec<&ClipItem> {
        self.selected_slots
            .iter()
            .filter_map(|slot| self.state.get(*slot))
            .collect()
    }

    pub(crate) fn filtered_actions(&self) -> Vec<PaletteAction> {
        let q = self.search_query.to_lowercase();
        let mut actions = Vec::new();

        // 1. Built-in ops
        let items = self.get_selected_items();
        let ops = get_applicable_ops(&items);
        for op in ops {
            if q.is_empty()
                || op.name.to_lowercase().contains(&q)
                || op.label.to_lowercase().contains(&q)
                || op.description.to_lowercase().contains(&q)
                || op.category.to_lowercase().contains(&q)
            {
                actions.push(PaletteAction::Op(op));
            }
        }

        // 2. Discovered Recipes
        for r in &self.recipes {
            if !r.is_applicable(&items) {
                continue;
            }
            if q.is_empty()
                || r.name.to_lowercase().contains(&q)
                || r.description.to_lowercase().contains(&q)
                || r.tags.iter().any(|t| t.to_lowercase().contains(&q))
                || q.starts_with('/')
            {
                actions.push(PaletteAction::Recipe(r.clone()));
            }
        }

        actions
    }

    pub(crate) fn execute_current_action(&mut self, ctx: &egui::Context) {
        self.execute_current_action_with(ctx, |request| {
            send_ipc_request(request).map_err(|error| error.to_string())
        });
    }

    pub(crate) fn execute_current_action_with(
        &mut self,
        ctx: &egui::Context,
        mut send: impl FnMut(&IpcRequest) -> Result<IpcResponse, String>,
    ) {
        let actions = self.filtered_actions();
        if let Some(act) = actions.get(self.selected_action_index) {
            match act {
                PaletteAction::Op(op) => {
                    if op.name == "diff" {
                        self.open_diff_viewer();
                        return;
                    }
                    if op.name == "binary-diff" {
                        self.open_binary_diff_viewer();
                        return;
                    }

                    let req = IpcRequest::ExecuteOp {
                        op_name: op.name.to_string(),
                        secondary_slot: None,
                        selected_slots: Some(self.selected_slots.clone()),
                        separator: None,
                    };
                    self.execute_action_request_with(ctx, &req, true, &mut send);
                }
                PaletteAction::Recipe(r) => {
                    if let Some((msg, def)) = recipe_needs_prompt(r, &self.state) {
                        self.in_app_prompt = Some(InAppPromptState {
                            recipe_name: r.name.clone(),
                            message: msg,
                            input_text: def,
                            first_frame: true,
                        });
                    } else {
                        let req = IpcRequest::ExecuteRecipe {
                            recipe_name: r.name.clone(),
                        };
                        self.execute_action_request_with(ctx, &req, true, &mut send);
                    }
                }
            }
        }
    }

    pub(crate) fn execute_action_request_with(
        &mut self,
        ctx: &egui::Context,
        request: &IpcRequest,
        close_on_success: bool,
        send: &mut impl FnMut(&IpcRequest) -> Result<IpcResponse, String>,
    ) -> bool {
        match send(request) {
            Ok(IpcResponse::OpResult { .. } | IpcResponse::Success { .. }) => {
                if close_on_success {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                true
            }
            Ok(IpcResponse::Error { error }) => {
                self.set_status_message(format!("Error: {error}"));
                false
            }
            Ok(_) => {
                self.set_status_message("Error: unexpected response from daemon");
                false
            }
            Err(error) => {
                self.set_status_message(format!("Error: {error}"));
                false
            }
        }
    }

    pub(crate) fn execute_auto_action_change_with(
        &mut self,
        ctx: &egui::Context,
        previous_enabled: bool,
        enabled: bool,
        send: &mut impl FnMut(&IpcRequest) -> Result<IpcResponse, String>,
    ) {
        let request = IpcRequest::SetAutoAction {
            key: "trim_link_tracking".to_owned(),
            enabled,
        };
        if !self.execute_action_request_with(ctx, &request, false, send) {
            self.config.auto_actions.trim_link_tracking = previous_enabled;
        }
    }
}
