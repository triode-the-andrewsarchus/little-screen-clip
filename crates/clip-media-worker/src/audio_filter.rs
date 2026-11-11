//! Audio graphs without file-opening or command-source filters.
use ffmpeg_next as ffmpeg;
use std::ffi::{CStr, CString};

struct Parsed(*mut ffmpeg::ffi::AVFilterGraphSegment);
impl Drop for Parsed {
    fn drop(&mut self) {
        // SAFETY: this pointer is owned by the successful segment parse below.
        unsafe { ffmpeg::ffi::avfilter_graph_segment_free(&mut self.0) };
    }
}

pub(super) fn validate(graph: &mut ffmpeg::filter::Graph, spec: &str) -> Result<(), String> {
    let spec = CString::new(spec).map_err(|_| "NUL in audio filter graph")?;
    let mut parsed = Parsed(std::ptr::null_mut());
    // SAFETY: graph and the NUL-terminated description are live. This only
    // parses descriptions; no filters are instantiated or initialized yet.
    let result = unsafe {
        ffmpeg::ffi::avfilter_graph_segment_parse(
            graph.as_mut_ptr(),
            spec.as_ptr(),
            0,
            &mut parsed.0,
        )
    };
    if result < 0 {
        return Err(ffmpeg::Error::from(result).to_string());
    }
    let mut count = 0;
    // SAFETY: successful parsing owns all arrays/strings through Parsed until
    // this function returns. Indices stay within the parser-provided lengths.
    unsafe {
        for index in 0..(*parsed.0).nb_chains {
            let chain = &**(*parsed.0).chains.add(index);
            for index in 0..chain.nb_filters {
                count += 1;
                if count > 512 {
                    return Err("Audio filter graph exceeds the filter budget".into());
                }
                let filter = &**chain.filters.add(index);
                let name = CStr::from_ptr(filter.filter_name).to_bytes();
                if !ALLOWED.contains(&name) {
                    return Err(format!(
                        "Unsupported audio filter: {}",
                        String::from_utf8_lossy(name)
                    ));
                }
            }
        }
    }
    Ok(())
}

const ALLOWED: &[&[u8]] = &[
    b"anull",
    b"aformat",
    b"aresample",
    b"volume",
    b"asoftclip",
    b"alimiter",
    b"lowpass",
    b"highpass",
    b"pan",
    b"aeval",
    b"asplit",
    b"amix",
    b"concat",
    b"atrim",
    b"asetpts",
    b"afade",
    b"loudnorm",
    b"silenceremove",
    b"areverse",
];

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filter_descriptions_are_checked_before_sources_can_open_files() {
        let mut graph = ffmpeg::filter::Graph::new();
        validate(
            &mut graph,
            "[0:a]aeval='val(1)|val(0)':c=stereo,volume='if(between(t,1,2),0.5,1)',asoftclip[out]",
        )
        .unwrap();
        for spec in [
            "amovie=/etc/passwd[out]",
            "anullsrc[out]",
            "asendcmd=f=/etc/passwd,anull",
        ] {
            assert!(
                validate(&mut graph, spec)
                    .unwrap_err()
                    .contains("Unsupported audio filter")
            );
        }
    }
}
