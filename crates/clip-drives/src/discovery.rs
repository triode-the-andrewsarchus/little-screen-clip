//! Bounded read-only block inventory, including partition and hardware evidence.
use crate::{Drive, Result};
use little_clip_core::process::CommandRequest;
use serde_json::Value;
use std::{collections::HashMap, time::Duration};

pub fn discover() -> Result<Vec<Drive>> {
    let output=CommandRequest::new("lsblk").args(["--bytes","--json","--paths","--output","NAME,PATH,TYPE,SIZE,RM,HOTPLUG,RO,FSTYPE,UUID,PARTUUID,LABEL,MOUNTPOINTS,SERIAL,WWN,START"]).timeout(Duration::from_secs(5)).max_output_bytes(2*1024*1024).own_process_group().run().map_err(|e| e.to_string())?;
    if !output.status.success() || output.stdout_truncated {
        return Err("Block inventory failed or exceeded its limit".into());
    }
    let mut drives = parse(&serde_json::from_slice::<Value>(&output.stdout)?)?;
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
                std::fs::metadata(&mount.source)
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
            drive.read_only |= mount.read_only;
        } else {
            drive.mount = None;
        }
    }
    Ok(drives)
}
fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}
fn number(v: &Value, key: &str) -> u64 {
    v[key]
        .as_u64()
        .or_else(|| v[key].as_str()?.parse().ok())
        .unwrap_or(0)
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
        append(parent, parent, &mut drives);
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
fn append(v: &Value, parent: &Value, drives: &mut Vec<Drive>) {
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
            removable: flag(parent, "rm") || flag(parent, "hotplug"),
            read_only: flag(v, "ro"),
            evidence,
            ambiguous: false,
        });
    }
    if let Some(children) = children {
        for child in children {
            append(child, parent, drives);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inventories_numeric_capacity_and_refuses_duplicate_partition_ids() {
        let tree = serde_json::json!({"blockdevices":[{"type":"disk","size":3000000000_u64,"rm":true,"serial":"first","children":[{"type":"part","path":"/dev/sdb1","size":2000000000,"fstype":"vfat","uuid":"fat","partuuid":"same","mountpoints":["/media/a"]}]},{"type":"disk","size":3000000000_u64,"rm":true,"serial":"second","children":[{"type":"part","path":"/dev/sdc1","size":2000000000,"fstype":"vfat","uuid":"fat","partuuid":"same"}]}]});
        let drives = parse(&tree).unwrap();
        assert_eq!(drives.len(), 2);
        assert!(drives.iter().all(|d| d.ambiguous));
        assert_eq!(drives[0].drive_capacity, 3000000000);
        assert!(drives[0].removable);
        assert_ne!(drives[0].evidence, drives[1].evidence);
    }
}
