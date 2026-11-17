//! Bounded read-only block inventory, including partition and hardware evidence.
use crate::{Drive, Result};
use little_clip_core::process::{CommandOutput, CommandRequest};
use serde_json::Value;
use std::{collections::HashMap, time::Duration};

pub fn discover() -> Result<Vec<Drive>> {
    let value = inventory(|columns| {
        CommandRequest::new("lsblk")
            .args(["--bytes", "--json", "--paths", "--output", columns])
            .timeout(Duration::from_secs(5))
            .max_output_bytes(2 * 1024 * 1024)
            .own_process_group()
            .run()
            .map_err(|e| crate::Error::from(e.to_string()))
    })?;
    let mut drives = parse(&value)?;
    let mounts = crate::mounts::inventory()?;
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    for drive in &mut drives {
        if drive.mount.is_none() {
            continue;
        }
        let device = std::fs::metadata(&drive.device)?.rdev();
        let candidates = mounts
            .iter()
            .filter(|mount| {
                mount.source.starts_with("/dev")
                    && std::fs::metadata(&mount.source)
                        .is_ok_and(|m| m.file_type().is_block_device() && m.rdev() == device)
            })
            .collect::<Vec<_>>();
        let selected = candidates
            .iter()
            .find(|mount| Some(&mount.target) == drive.mount.as_ref())
            .copied()
            .or_else(|| {
                if candidates.len() == 1 {
                    Some(candidates[0])
                } else {
                    candidates
                        .iter()
                        .find(|mount| mount.root == std::path::Path::new("/"))
                        .copied()
                }
            });
        if let Some(mount) = selected {
            drive.mount = Some(mount.target.clone());
            drive.read_only |= mount.read_only || mount.super_read_only;
        } else {
            drive.mount = None;
        }
    }
    Ok(drives)
}
fn inventory(mut query: impl FnMut(&str) -> Result<CommandOutput>) -> Result<Value> {
    let columns =
        "NAME,PATH,TYPE,SIZE,RM,HOTPLUG,RO,FSTYPE,UUID,PARTUUID,LABEL,MOUNTPOINTS,SERIAL,WWN,START";
    let layout_columns = "PARTN,PARTTYPE,PARTLABEL,PARTFLAGS,PTTYPE,PTUUID,LOG-SEC";
    let mut output = query(&format!("{columns},{layout_columns},DISK-SEQ"))?;
    if !output.status.success()
        && layout_columns
            .split(',')
            .any(|column| String::from_utf8_lossy(&output.stderr).contains(column))
    {
        // Optional layout columns vary with util-linux. Retain the existing
        // numeric/start observations instead of disabling all drive services.
        output = query(&format!("{columns},DISK-SEQ"))?;
    }
    // Older lsblk can still provide browsing/sync inventory. Imaging refuses
    // admission when this observation is absent rather than weakening its guard.
    if !output.status.success() && String::from_utf8_lossy(&output.stderr).contains("DISK-SEQ") {
        output = query(columns)?;
    }
    if !output.status.success() || output.stdout_truncated {
        return Err("Block inventory failed or exceeded its limit".into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}
fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}
fn number(v: &Value, key: &str) -> u64 {
    optional_number(v, key).unwrap_or(0)
}
fn optional_number(v: &Value, key: &str) -> Option<u64> {
    v[key].as_u64().or_else(|| v[key].as_str()?.parse().ok())
}
fn flag(v: &Value, key: &str) -> bool {
    v[key].as_bool().unwrap_or_else(|| number(v, key) != 0)
}
fn parse(value: &Value) -> Result<Vec<Drive>> {
    let devices = value["blockdevices"]
        .as_array()
        .ok_or("Invalid block inventory")?;
    let mut drives = Vec::new();
    for parent in devices {
        append(parent, parent, parent, &mut drives);
    }
    let mut counts = HashMap::new();
    for drive in &drives {
        *counts.entry(drive.id.clone()).or_insert(0) += 1;
    }
    for drive in &mut drives {
        drive.ambiguous = counts[&drive.id] > 1;
    }
    Ok(drives)
}
fn partition_layout(v: &Value, table: &Value) -> Option<Box<crate::PartitionLayout>> {
    if text(v, "type") != "part" {
        return None;
    }
    let optional_text = |v: &Value, key| {
        let value = text(v, key);
        (!value.is_empty()).then(|| value.to_owned())
    };
    let start = optional_number(v, "start");
    Some(Box::new(crate::PartitionLayout {
        number: optional_number(v, "partn"),
        start_512_sectors: start,
        // lsblk START is always in 512-byte sectors, including with --bytes.
        offset_bytes: start.and_then(|start| start.checked_mul(512)),
        logical_sector_bytes: optional_number(v, "log-sec")
            .or_else(|| optional_number(table, "log-sec"))
            .filter(|size| *size != 0),
        partition_type: optional_text(v, "parttype"),
        label: optional_text(v, "partlabel"),
        flags: optional_text(v, "partflags"),
        table_type: optional_text(table, "pttype"),
        table_uuid: optional_text(table, "ptuuid"),
    }))
}
fn append(v: &Value, parent: &Value, table: &Value, drives: &mut Vec<Drive>) {
    let children = v["children"].as_array();
    let kind = text(v, "type");
    if matches!(kind, "part" | "crypt" | "rom") || (kind == "disk" && children.is_none()) {
        let part = text(v, "partuuid");
        let uuid = text(v, "uuid");
        let evidence = format!(
            "{}|{}|{}|{}|{}|{}",
            text(parent, "wwn"),
            text(parent, "serial"),
            number(parent, "size"),
            number(v, "start"),
            number(v, "size"),
            uuid
        );
        let id = if !part.is_empty() {
            format!("part:{part}")
        } else if !uuid.is_empty() {
            format!("fs:{uuid}")
        } else {
            format!("device:{}", text(v, "path"))
        };
        let mount = v["mountpoints"]
            .as_array()
            .and_then(|a| a.iter().find_map(Value::as_str))
            .filter(|p| !p.is_empty())
            .map(Into::into);
        drives.push(Drive {
            id,
            label: if text(v, "label").is_empty() {
                text(v, "path").into()
            } else {
                text(v, "label").into()
            },
            device: text(v, "path").into(),
            mount,
            part_uuid: (!part.is_empty()).then(|| part.into()),
            filesystem_uuid: (!uuid.is_empty()).then(|| uuid.into()),
            filesystem: text(v, "fstype").into(),
            capacity: number(v, "size"),
            drive_capacity: number(parent, "size"),
            disk_sequence: Some(number(v, "disk-seq")).filter(|value| *value != 0),
            partition_layout: partition_layout(v, table),
            removable: flag(parent, "rm") || flag(parent, "hotplug"),
            read_only: flag(v, "ro"),
            evidence,
            ambiguous: false,
        });
    }
    if let Some(children) = children {
        for child in children {
            append(child, parent, v, drives);
        }
    }
}

pub(crate) fn select(drives: Vec<Drive>, id: &str) -> Result<Drive> {
    let explicit = id.starts_with("/dev/");
    let mut matches = drives.into_iter().filter(|drive| {
        if explicit {
            drive.device == std::path::Path::new(id)
        } else {
            drive.id == id
        }
    });
    let drive = matches.next().ok_or("Drive is not available")?;
    if (!explicit && drive.ambiguous) || matches.next().is_some() {
        return Err(
            "Multiple drives have the same identity; automatic writes are suspended".into(),
        );
    }
    Ok(drive)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn older_lsblk_columns_fall_back_without_inventing_disk_lifetime_or_layout() {
        use std::os::unix::process::ExitStatusExt;
        for (unsupported, attempts) in [
            (Vec::new(), 1),
            (vec!["PARTN"], 2),
            (vec!["DISK-SEQ"], 2),
            (vec!["PARTN", "DISK-SEQ"], 3),
        ] {
            let mut requested = Vec::new();
            let value = inventory(|columns| {
                requested.push(columns.to_owned());
                let missing = columns.split(',').find(|column| unsupported.contains(column));
                Ok(CommandOutput {
                    status: std::process::ExitStatus::from_raw(if missing.is_some() { 256 } else { 0 }),
                    stdout: br#"{"blockdevices":[{"type":"part","path":"/dev/example1","start":2048,"size":1000}]}"#.to_vec(),
                    stderr: missing.map(|column| format!("unknown column: {column}").into_bytes()).unwrap_or_default(),
                    stdout_truncated: false,
                    stderr_truncated: false,
                })
            }).unwrap();
            assert_eq!(requested.len(), attempts, "{unsupported:?}");
            let drives = parse(&value).unwrap();
            assert_eq!(drives[0].disk_sequence, None);
            let layout = drives[0].partition_layout.as_ref().unwrap();
            assert_eq!(layout.offset_bytes, Some(1_048_576));
            assert_eq!(layout.number, None);
            assert_eq!(layout.table_uuid, None);
        }
    }
    #[test]
    fn partition_layout_retains_units_unknown_fields_and_immediate_table_owner() {
        let tree = serde_json::json!({"blockdevices":[{
            "type":"disk", "size":5000000000_u64, "pttype":"gpt", "ptuuid":"outer", "log-sec":4096,
            "children":[{
                "type":"part", "path":"/dev/example1", "start":"2048", "partn":1,
                "parttype":"type-id", "partlabel":"Embedded", "partflags":"0x80", "size":1000000,
                "pttype":"dos", "ptuuid":"inner", "children":[{
                    "type":"part", "path":"/dev/nested1", "start":18446744073709551615_u64,
                    "size":1000
                }]
            },{"type":"part", "path":"/dev/example2", "start":0,"size":1000},
            {"type":"crypt", "path":"/dev/mapper/example", "size":1000}]
        }]});
        let drives = parse(&tree).unwrap();
        let layout = drives[0].partition_layout.as_ref().unwrap();
        assert_eq!(layout.number, Some(1));
        assert_eq!(layout.start_512_sectors, Some(2048));
        assert_eq!(layout.offset_bytes, Some(1_048_576));
        assert_eq!(layout.logical_sector_bytes, Some(4096));
        assert_eq!(layout.table_type.as_deref(), Some("gpt"));
        assert_eq!(layout.table_uuid.as_deref(), Some("outer"));
        assert_eq!(layout.partition_type.as_deref(), Some("type-id"));
        assert_eq!(layout.label.as_deref(), Some("Embedded"));
        assert_eq!(layout.flags.as_deref(), Some("0x80"));
        let nested = drives[1].partition_layout.as_ref().unwrap();
        assert_eq!(nested.table_type.as_deref(), Some("dos"));
        assert_eq!(nested.table_uuid.as_deref(), Some("inner"));
        assert_eq!(nested.start_512_sectors, Some(u64::MAX));
        assert_eq!(nested.offset_bytes, None);
        assert_eq!(nested.number, None);
        assert_eq!(nested.logical_sector_bytes, None);
        let zero = drives[2].partition_layout.as_ref().unwrap();
        assert_eq!(zero.offset_bytes, Some(0));
        assert_eq!(zero.partition_type, None);
        assert!(drives[3].partition_layout.is_none());
        let mut legacy = serde_json::to_value(&drives[0]).unwrap();
        legacy.as_object_mut().unwrap().remove("partition_layout");
        assert!(
            serde_json::from_value::<Drive>(legacy)
                .unwrap()
                .partition_layout
                .is_none()
        );
    }
    #[test]
    fn inventories_numeric_capacity_and_refuses_duplicate_partition_ids() {
        let tree = serde_json::json!({"blockdevices":[{"type":"disk","size":3000000000_u64,"rm":true,"serial":"first","children":[{"type":"part","path":"/dev/sdb1","size":2000000000,"fstype":"vfat","uuid":"fat","partuuid":"same","disk-seq":17,"mountpoints":["/media/a"]}]},{"type":"disk","size":3000000000_u64,"rm":true,"serial":"second","children":[{"type":"part","path":"/dev/sdc1","size":2000000000,"fstype":"vfat","uuid":"fat","partuuid":"same","disk-seq":"18"}]}]});
        let drives = parse(&tree).unwrap();
        assert_eq!(drives.len(), 2);
        assert!(drives.iter().all(|d| d.ambiguous));
        assert_eq!(drives[0].drive_capacity, 3000000000);
        assert_eq!(drives[0].disk_sequence, Some(17));
        assert_eq!(drives[1].disk_sequence, Some(18));
        let mut reconnected = tree.clone();
        reconnected["blockdevices"][0]["children"][0]["disk-seq"] = 19.into();
        let next = parse(&reconnected).unwrap();
        assert_eq!(next[0].evidence, drives[0].evidence);
        assert_ne!(next[0], drives[0]);
        let mut old_record = serde_json::to_value(&drives[0]).unwrap();
        old_record.as_object_mut().unwrap().remove("disk_sequence");
        assert_eq!(
            serde_json::from_value::<Drive>(old_record)
                .unwrap()
                .disk_sequence,
            None
        );
        assert!(drives[0].removable);
        assert_ne!(drives[0].evidence, drives[1].evidence);
    }
}
