use excalidraw_render::{InkCommand, InkPath};
#[test]
fn interactive_ink_uses_real_pressure_and_rejects_invalid_input() {
    let soft = InkPath::prepare(&[[0., 0., 0.1], [30., 0., 0.1], [60., 0., 0.1]], 2.).unwrap();
    let hard = InkPath::prepare(&[[0., 0., 0.9], [30., 0., 0.9], [60., 0., 0.9]], 2.).unwrap();
    let height = |path: &InkPath| {
        path.commands
            .iter()
            .filter_map(|command| match command {
                InkCommand::Move(p) | InkCommand::Line(p) => Some(p[1].abs()),
                InkCommand::Quadratic { control, end } => Some(control[1].abs().max(end[1].abs())),
                InkCommand::Close => None,
            })
            .fold(0., f64::max)
    };
    assert!(height(&hard) > height(&soft) * 1.5);
    assert!(InkPath::prepare(&[[0., 0., f64::NAN]; 2], 2.).is_err());
    assert!(InkPath::prepare(&[[0., 0., 1.1]; 2], 2.).is_err());
    assert!(InkPath::prepare(&vec![[0., 0., 0.5]; 4097], 2.).is_err());
    assert!(InkPath::prepare(&[[0., 0., 0.5]; 2], 2.).is_ok());
}
