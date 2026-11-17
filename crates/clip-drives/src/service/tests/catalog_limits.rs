//! One-run budgets retain daemon defaults and the existing bulk worker admission.
use super::*;

struct Capture {
    fixture: TestDrives,
    _media: Option<tempfile::TempDir>,
    service: Service,
    drive: Drive,
}
impl Capture {
    fn new() -> Self {
        let fixture = TestDrives::new(0);
        let media = std::env::var_os("LITTLE_CLIP_DRIVE_TEST_SOURCE_DIR").map(|_| {
            let (media, _, drive) = crate::sync_edge_tests::native_fs::fixture();
            fixture.drives.lock().unwrap()[1] = drive;
            media
        });
        let drive = fixture.drives.lock().unwrap()[1].clone();
        let source = drive.mount.as_ref().unwrap();
        fs::create_dir_all(source.join("nested/deep")).unwrap();
        for index in 0..25 {
            fs::write(
                source.join(format!("file-{index:02}")),
                b"payload stays on media",
            )
            .unwrap();
        }
        fs::write(source.join("nested/deep/leaf"), b"nested payload").unwrap();
        let service = fixture.start_configured(
            DriveConfig {
                catalog_max_entries: 5,
                catalog_max_bytes: 2 * 1024 * 1024,
                catalog_max_depth: 4,
                catalog_timeout_seconds: 30,
                ..Default::default()
            },
            None,
            partition::Authority::default(),
        );
        Self {
            fixture,
            _media: media,
            service,
            drive,
        }
    }
    fn capture(&self, limits: Option<CatalogLimitOverrides>) -> Catalog {
        let before: Vec<_> = status(&self.service)
            .catalogs
            .into_iter()
            .map(|c| c.id)
            .collect();
        let drive = self.drive.id.clone();
        let request = match limits {
            Some(limits) => DriveRequest::CatalogWithLimits { drive, limits },
            None => DriveRequest::Catalog { drive },
        };
        finish(&self.service, request);
        let mut added: Vec<_> = status(&self.service)
            .catalogs
            .into_iter()
            .filter(|c| !before.contains(&c.id))
            .collect();
        assert_eq!(added.len(), 1);
        added.pop().unwrap()
    }
}

#[test]
fn explicit_larger_capture_inherits_custom_defaults_records_limits_and_preserves_later_caps() {
    let capture = Capture::new();
    let defaults = status(&capture.service).config;
    let limited = capture.capture(None);
    assert!(!limited.complete && limited.entries <= 5);
    assert_eq!(limited.limits, Some((&defaults).into()));
    let full = capture.capture(Some(CatalogLimitOverrides {
        entries: Some(2000),
        ..Default::default()
    }));
    assert!(full.complete, "{}", full.coverage);
    assert!(full.entries > 25);
    let expected = CatalogLimits {
        entries: 2000,
        ..(&defaults).into()
    };
    assert_eq!(full.limits, Some(expected));
    let db = Store::existing(
        &capture
            .fixture
            .temp
            .path()
            .join("state/catalogs")
            .join(format!("{}.sqlite", full.id)),
        true,
    )
    .unwrap();
    let saved: Catalog = db.get("catalog", &full.id).unwrap().unwrap();
    assert_eq!(saved.limits, Some(expected));
    let leaf = scan::entry(&db, "catalog", Path::new("nested/deep/leaf"))
        .unwrap()
        .unwrap();
    assert_eq!(leaf.size, b"nested payload".len() as u64);
    assert_eq!(leaf.hash, None);
    assert_eq!(
        serde_json::to_value(status(&capture.service).config).unwrap(),
        serde_json::to_value(defaults).unwrap()
    );
    let limited_again = capture.capture(None);
    assert!(!limited_again.complete && limited_again.entries <= 5);
    assert_eq!(limited_again.limits, limited.limits);
    assert_eq!(
        fs::read(capture.drive.mount.unwrap().join("nested/deep/leaf")).unwrap(),
        b"nested payload"
    );
    capture.service.shutdown().unwrap();
}

#[test]
fn invalid_per_run_limits_fail_before_creating_a_catalog_or_changing_preferences() {
    let capture = Capture::new();
    let original = serde_json::to_value(status(&capture.service).config).unwrap();
    for limits in [
        CatalogLimitOverrides {
            bytes: Some(0),
            ..Default::default()
        },
        CatalogLimitOverrides {
            bytes: Some(u64::MAX),
            ..Default::default()
        },
        CatalogLimitOverrides {
            depth: Some(0),
            ..Default::default()
        },
        CatalogLimitOverrides {
            timeout_seconds: Some(0),
            ..Default::default()
        },
    ] {
        let id = accept(
            &capture.service,
            DriveRequest::CatalogWithLimits {
                drive: capture.drive.id.clone(),
                limits,
            },
        );
        let operation = wait_operation(&capture.service, &id, |op| op.finished.is_some());
        assert_eq!(operation.state, "failed", "{}", operation.message);
        assert!(
            operation.message.contains("Catalog"),
            "{}",
            operation.message
        );
        assert!(status(&capture.service).catalogs.is_empty());
        assert!(!capture.fixture.temp.path().join("state/catalogs").exists());
    }
    assert_eq!(
        serde_json::to_value(status(&capture.service).config).unwrap(),
        original
    );
    capture.service.shutdown().unwrap();
}
