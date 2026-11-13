use super::*;

impl Scene {
    pub(super) fn create(
        &self,
        document: &mut Document,
        id: String,
        kind: Kind,
        bounds: Bounds,
        points: Vec<[f64; 2]>,
        style: Style,
    ) -> Result<(), Error> {
        let values = elements(document)?;
        if values.len() >= MAX_ELEMENTS
            || id.is_empty()
            || id.len() > 128
            || values
                .iter()
                .any(|value| value.get("id").and_then(Value::as_str) == Some(&id))
        {
            return Err(invalid(
                "A new element needs an unused identity and space in the scene",
            ));
        }
        if !bounds.valid() || !TOOLS.contains(&kind) {
            return Err(invalid("Unsupported creation tool or invalid geometry"));
        }
        let path = PATH_KINDS.contains(&kind);
        let mut shape = Element::new(
            kind,
            Profile::V0_18_1,
            id.as_str().into(),
            timestamp().into(),
        )?;
        set_bounds(&mut shape, bounds)?;
        style.apply(&mut shape)?;
        shape.set_raw(
            "seed",
            json!((uuid::Uuid::new_v4().as_u128() as u32) & 0x7fff_ffff),
        );
        if path {
            set_path(&mut shape, points)?;
        }
        let mut values = values.to_vec();
        values.push(Value::Object(shape.into_object()));
        document.set_root("elements", Value::Array(values))
    }

    pub(super) fn move_items(
        &self,
        document: &mut Document,
        ids: &[String],
        by: [f64; 2],
    ) -> Result<(), Error> {
        let movement = self.movement(ids, by)?;
        self.apply_movement(document, &movement)
    }

    pub(super) fn apply_movement(
        &self,
        document: &mut Document,
        movement: &Movement,
    ) -> Result<(), Error> {
        for item in &self.items {
            let path = movement.paths.get(&item.id);
            if path.is_none() && movement.offsets.get(&item.id) == Some(&[0.0; 2]) {
                continue;
            }
            let position = path.map(|path| path.origin).or_else(|| {
                movement
                    .offsets
                    .get(&item.id)
                    .map(|[x, y]| [item.bounds.x + x, item.bounds.y + y])
            });
            let Some(position) = position else {
                continue;
            };
            if position
                .into_iter()
                .any(|v| !v.is_finite() || v.abs() > MAX_COORDINATE)
            {
                return Err(invalid("Element exceeds the canvas coordinate limits"));
            }
            edit(document, item.index, |shape| {
                for (key, value) in [(element::X, position[0]), (element::Y, position[1])] {
                    shape.set(key, Number::from_f64(value)?)?;
                }
                if let Some(path) = path {
                    set_path(shape, path.points.clone())?;
                }
                Ok(())
            })?;
        }
        Ok(())
    }

    /// Resolves a drag without changing source, versions, or editor history.
    ///
    /// # Errors
    ///
    /// Returns an error for locked dependents or unsupported path routing,
    /// exactly as committing the same move would.
    pub fn movement(&self, ids: &[String], by: [f64; 2]) -> Result<Movement, Error> {
        let mut movement = Movement::default();
        if !by
            .into_iter()
            .all(|v| v.is_finite() && v.abs() <= MAX_COORDINATE)
        {
            return Err(invalid("Invalid movement"));
        }
        if by == [0.0; 2] {
            return Ok(movement);
        }
        let selected = self.selection(ids)?;
        let original = elements(&self.document)?;
        for item in &self.items {
            if selected.contains(&item.index)
                && item.kind != Kind::Arrow
                && arrow_container(&original[item.index], &self.items).is_none()
            {
                movement.offsets.insert(item.id.clone(), by);
            }
        }
        let moved_ids: HashSet<_> = selected
            .iter()
            .filter_map(|i| original[*i].get("id").and_then(Value::as_str))
            .collect();
        for arrow in self.items.iter().filter(|item| item.kind == Kind::Arrow) {
            self.move_arrow(&mut movement, arrow, &selected, &moved_ids, by)?;
        }
        Ok(movement)
    }

    fn move_arrow(
        &self,
        movement: &mut Movement,
        arrow: &Item,
        selected: &BTreeSet<usize>,
        moved_ids: &HashSet<&str>,
        by: [f64; 2],
    ) -> Result<(), Error> {
        let value = &elements(&self.document)?[arrow.index];
        let bindings = [binding(value, "startBinding"), binding(value, "endBinding")];
        let endpoint_moved = bindings.map(|id| id.is_some_and(|id| moved_ids.contains(id)));
        let moving = selected.contains(&arrow.index);
        if !moving && !endpoint_moved.into_iter().any(|moved| moved) {
            return Ok(());
        }
        if arrow.locked {
            return Err(invalid("A connected arrow is locked"));
        }
        let base = if moving { by } else { [0.0; 2] };
        let endpoint_delta = bindings.map(|id| {
            id.map_or(
                base,
                |id| {
                    if moved_ids.contains(id) { by } else { [0.0; 2] }
                },
            )
        });
        if endpoint_delta == [base, base]
            || bindings.iter().all(Option::is_some) && endpoint_delta[0] == endpoint_delta[1]
        {
            let delta = endpoint_delta[0];
            if delta == [0.0; 2] {
                return Ok(());
            }
            movement.offsets.insert(arrow.id.clone(), delta);
            return self.move_arrow_labels(movement, arrow, delta);
        }
        self.stretch_arrow(movement, arrow, value, base, endpoint_delta)
    }

    pub(super) fn stretch_arrow(
        &self,
        movement: &mut Movement,
        arrow: &Item,
        value: &Value,
        base: [f64; 2],
        endpoint_delta: [[f64; 2]; 2],
    ) -> Result<(), Error> {
        if arrow.bounds.angle != 0.0
            || value.get("elbowed").and_then(Value::as_bool) == Some(true)
            || value.get("fixedSegments").is_some_and(|v| !v.is_null())
        {
            // Rigid movement above is safe for every path. Stretching these
            // paths needs native routing; failure rolls back the whole gesture.
            return Err(invalid(
                "Connected rotated or elbowed arrows need source editing",
            ));
        }
        let old = points(value)?;
        let mut world: Vec<_> = old
            .iter()
            .map(|point| {
                [
                    arrow.bounds.x + point[0] + base[0],
                    arrow.bounds.y + point[1] + base[1],
                ]
            })
            .collect();
        let last = world.len() - 1;
        for (endpoint, index) in [0, last].into_iter().enumerate() {
            world[index] = [
                arrow.bounds.x + old[index][0] + endpoint_delta[endpoint][0],
                arrow.bounds.y + old[index][1] + endpoint_delta[endpoint][1],
            ];
        }
        let origin = world[0];
        let center_delta = [
            (world[0][0] + world[last][0] - old[0][0] - old[last][0]) / 2.0 - arrow.bounds.x,
            (world[0][1] + world[last][1] - old[0][1] - old[last][1]) / 2.0 - arrow.bounds.y,
        ];
        let local = world
            .into_iter()
            .map(|point| [point[0] - origin[0], point[1] - origin[1]])
            .collect();
        movement.paths.insert(
            arrow.id.clone(),
            MovedPath {
                origin,
                points: local,
            },
        );
        self.move_arrow_labels(movement, arrow, center_delta)
    }

    fn move_arrow_labels(
        &self,
        movement: &mut Movement,
        arrow: &Item,
        delta: [f64; 2],
    ) -> Result<(), Error> {
        if delta == [0.0; 2] {
            return Ok(());
        }
        let original = elements(&self.document)?;
        for label in &self.items {
            if original[label.index]
                .get("containerId")
                .and_then(Value::as_str)
                == Some(&arrow.id)
            {
                if label.locked {
                    return Err(invalid("A connected label is locked"));
                }
                movement.offsets.insert(label.id.clone(), delta);
            }
        }
        Ok(())
    }

    pub(super) fn delete_items(
        &self,
        document: &mut Document,
        ids: &[String],
    ) -> Result<(), Error> {
        let selected = self.selection(ids)?;
        let original = elements(&self.document)?;
        let deleted: HashSet<_> = selected
            .iter()
            .filter_map(|i| original[*i].get("id").and_then(Value::as_str))
            .collect();
        for index in &selected {
            edit(document, *index, |shape| {
                shape.set_raw("isDeleted", json!(true));
                Ok(())
            })?;
        }
        for (index, value) in original.iter().enumerate() {
            if selected.contains(&index) || !value.is_object() {
                continue;
            }
            edit(document, index, |shape| {
                for key in ["startBinding", "endBinding"] {
                    if binding(value, key).is_some_and(|id| deleted.contains(id)) {
                        shape.set_raw(key, Value::Null);
                    }
                }
                if let Some(bound) = value.get("boundElements").and_then(Value::as_array) {
                    let kept: Vec<_> = bound
                        .iter()
                        .filter(|entry| {
                            !entry
                                .get("id")
                                .and_then(Value::as_str)
                                .is_some_and(|id| deleted.contains(id))
                        })
                        .cloned()
                        .collect();
                    if kept.len() != bound.len() {
                        shape.set_raw("boundElements", Value::Array(kept));
                    }
                }
                Ok(())
            })?;
        }
        Ok(())
    }
}

fn arrow_container<'a>(value: &Value, items: &'a [Item]) -> Option<&'a Item> {
    let id = value.get("containerId")?.as_str()?;
    items
        .iter()
        .find(|item| item.id == id && item.kind == Kind::Arrow)
}
pub(super) fn set_bounds(shape: &mut Element, bounds: Bounds) -> Result<(), Error> {
    for (key, value) in [
        (element::X, bounds.x),
        (element::Y, bounds.y),
        (element::WIDTH, bounds.width),
        (element::HEIGHT, bounds.height),
        (element::ANGLE, bounds.angle),
    ] {
        shape.set(key, Number::from_f64(value)?)?;
    }
    Ok(())
}
pub(super) fn binding<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.get("elementId")?.as_str()
}
pub(super) fn points(value: &Value) -> Result<Vec<[f64; 2]>, Error> {
    let points = value
        .get("points")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("Arrow has no usable points"))?;
    if !(2..=16_384).contains(&points.len()) {
        return Err(invalid("Arrow path exceeds the canvas limits"));
    }
    points
        .iter()
        .map(|value| read_point(value).ok_or_else(|| invalid("Invalid arrow point")))
        .collect()
}
pub(super) fn set_path(shape: &mut Element, points: Vec<[f64; 2]>) -> Result<(), Error> {
    shape.set_path(path_values(points)?)
}
pub(super) fn path_values(points: Vec<[f64; 2]>) -> Result<Vec<[Number; 2]>, Error> {
    if !(2..=16_384).contains(&points.len())
        || points
            .iter()
            .flatten()
            .any(|v| !v.is_finite() || v.abs() > MAX_COORDINATE)
    {
        return Err(invalid("Stroke exceeds the canvas limits"));
    }
    points
        .into_iter()
        .map(|[x, y]| Ok([Number::from_f64(x)?, Number::from_f64(y)?]))
        .collect()
}
