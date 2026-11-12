//! Canonical WaveJSON layout in a bounded, hostless embedded interpreter.
use rquickjs::{CatchResultExt, Context, Function, Runtime};
use std::time::{Duration, Instant};

const ENGINE: &str = include_str!("../../../../vendor/wavedrom/wavedrom.unpkg.js");
const SKINS: &[&str] = &[
    include_str!("../../../../vendor/wavedrom/skins/narrow.js"),
    include_str!("../../../../vendor/wavedrom/skins/dark.js"),
    include_str!("../../../../vendor/wavedrom/skins/lowkey.js"),
    include_str!("../../../../vendor/wavedrom/skins/narrower.js"),
    include_str!("../../../../vendor/wavedrom/skins/narrowerer.js"),
];
const BRIDGE: &str = r#"
globalThis.__renderWave = (() => {
    const render = wavedrom.renderAny;
    const stringify = wavedrom.onml.stringify;
    const skins = Object.assign({}, wavedrom.waveSkin, WaveSkin);
    return data => {
        const source = JSON.parse(data);
        const svg = render(0, source, skins);
        // The signal template hardcodes white paper even for the dark skin.
        // Change that background in our adapter before outlining/exporting.
        if (source.signal && source.config && source.config.skin === 'dark') {
            const body = svg.find(node => Array.isArray(node) && node[1] && node[1].id === 'waves_0');
            if (!body || !body[2] || body[2][0] !== 'rect') throw new Error('Missing signal paper');
            body[2][1].style = 'stroke:none;fill:#111';
        }
        return stringify(svg);
    };
})();
globalThis.wavedrom = undefined;
globalThis.WaveSkin = undefined;
"#;

pub(super) fn svg(source: &str) -> Result<(String, Vec<String>), String> {
    let value = little_clip_core::diagram::wavejson::parse(source)
        .map_err(|error| error.chars().take(2048).collect::<String>())?;
    let input = serde_json::to_string(&value).map_err(|error| error.to_string())?;
    let runtime = Runtime::new().map_err(|error| error.to_string())?;
    runtime.set_memory_limit(128 * 1024 * 1024);
    runtime.set_max_stack_size(512 * 1024);
    let started = Instant::now();
    runtime.set_interrupt_handler(Some(Box::new(move || {
        started.elapsed() > Duration::from_secs(3)
    })));
    // No module loader, QuickJS libc, filesystem, network, DOM, or script-visible host bindings
    // are installed. Only trusted bundled code is evaluated. Source is parsed as
    // Rust JSON5 data and passed as a function argument, never evaluated as code.
    let context = Context::full(&runtime).map_err(|error| error.to_string())?;
    let svg: String = context.with(|context| {
        (|| -> rquickjs::Result<String> {
            context.eval::<(), _>(ENGINE)?;
            for skin in SKINS {
                context.eval::<(), _>(*skin)?;
            }
            context.eval::<(), _>(BRIDGE)?;
            let render: Function = context.globals().get("__renderWave")?;
            render.call((input.as_str(),))
        })()
        .catch(&context)
        .map_err(|error| {
            format!(
                "WaveDrom: {}",
                error.to_string().chars().take(2048).collect::<String>()
            )
        })
    })?;
    // Outline generated SVG, isolating references and discarding executable or
    // unsupported SVG content before it can become an exportable artifact.
    super::compose_svg(
        [svg],
        super::fonts::metric_fonts(),
        "Arimo",
        super::ImagePolicy::Block,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_core::diagram::Format;

    fn width(svg: &str) -> f32 {
        roxmltree::Document::parse(svg)
            .unwrap()
            .root_element()
            .attribute("width")
            .unwrap()
            .parse()
            .unwrap()
    }

    #[test]
    fn timing_register_and_logic_figures_render_without_a_browser_or_node() {
        for source in [
            "{signal:[{name:'clock',wave:'p...'},{name:'bus',wave:'x34x',data:['A','B']}],head:{text:'Transaction',tick:0}}",
            "{reg:[{bits:4,name:'op'},{bits:12,name:'payload'}]}",
            "{assign:[['out',['&','a','b']]]}",
        ] {
            let rendered = super::super::render_inline(Format::Wavedrom, source, None).unwrap();
            assert!(rendered.svg.contains("<path"));
            assert!(rendered.png.len() > 1000);
            assert!(!rendered.svg.contains("<script"));
            assert!(rendered.warnings.is_empty(), "{:?}", rendered.warnings);
        }
    }

    #[test]
    fn fractional_periods_and_requested_skins_change_real_geometry() {
        let source = "{signal:[{name:'clk',wave:'01010',period:1}]}";
        let normal = svg(source).unwrap().0;
        let fractional = svg(&source.replace("period:1", "period:.5")).unwrap().0;
        let narrow = svg(&source.replace("}]}", "}],config:{skin:'narrow'}}"))
            .unwrap()
            .0;
        assert!(width(&fractional) < width(&normal));
        assert!(width(&narrow) < width(&normal));
        for skin in ["dark", "lowkey", "narrower", "narrowerer"] {
            let source = source.replace("}]}", &format!("}}],config:{{skin:'{skin}'}}}}"));
            assert!(svg(&source).is_ok(), "{skin}");
        }
    }

    #[test]
    fn dark_signal_skin_keeps_dark_paper_in_exported_vectors_and_pixels() {
        let rendered = super::super::render_inline(
            Format::Wavedrom,
            "{signal:[{name:'clock',wave:'01010'}],config:{skin:'dark'}}",
            None,
        )
        .unwrap();
        let image = image::load_from_memory(&rendered.png).unwrap().to_rgba8();
        let dark = image
            .pixels()
            .filter(|pixel| pixel.0[..3].iter().all(|channel| *channel < 40))
            .count();
        assert!(dark > (image.width() * image.height()) as usize / 2);
        assert!(rendered.svg.contains("#111111"));
    }

    #[test]
    fn source_is_not_evaluated_as_javascript_and_figures_are_isolated() {
        assert!(svg("{signal:(()=>{throw 'executed'})()}").is_err());
        assert!(svg("{signal:[{name:'a',wave:'01'}],config:{skin:'file:///tmp/skin'}}").is_err());
        let source = "{signal:[{name:'a',wave:'0101'}]}";
        let first = svg(source).unwrap();
        svg("{signal:[{name:'different',wave:'p....'}],head:{text:'header'},config:{skin:'narrow'}}").unwrap();
        assert_eq!(first, svg(source).unwrap());
    }
}
