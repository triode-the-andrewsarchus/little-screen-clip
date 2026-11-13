//! Shared resource boundary for trusted diagram bundles and data-only calls.
use rquickjs::{CatchResultExt, Context, Function, Runtime};
use std::time::{Duration, Instant};

pub(super) struct Program<'a> {
    pub name: &'a str,
    pub stack_bytes: usize,
    pub scripts: &'a [&'a str],
    pub bridge: &'a str,
    pub entry: &'a str,
}
impl Program<'_> {
    pub fn call(&self, input: &str) -> Result<String, String> {
        // Interpreter stack use varies with the embedded language (SCI has
        // deeper frames than the diagram-only bundles). Give QuickJS a known
        // native stack even when called from Rust's smaller test/worker threads.
        std::thread::scope(|scope| {
            std::thread::Builder::new()
                .name("diagram-script".into())
                .stack_size(self.stack_bytes + 1024 * 1024)
                .spawn_scoped(scope, || self.evaluate(input))
                .map_err(|error| format!("{}: {error}", self.name))?
                .join()
                .map_err(|_| format!("{}: interpreter thread failed", self.name))?
        })
    }

    fn evaluate(&self, input: &str) -> Result<String, String> {
        let runtime = Runtime::new().map_err(|error| error.to_string())?;
        runtime.set_memory_limit(128 * 1024 * 1024);
        runtime.set_max_stack_size(self.stack_bytes);
        let started = Instant::now();
        runtime.set_interrupt_handler(Some(Box::new(move || {
            started.elapsed() > Duration::from_secs(3)
        })));
        // No module loader, libc, filesystem, network, DOM or host bindings.
        // Only pinned trusted bundles and our bridge are evaluated; untrusted
        // diagram source is always a string argument, never script source.
        let context = Context::full(&runtime).map_err(|error| error.to_string())?;
        let output: String = context.with(|context| {
            (|| -> rquickjs::Result<String> {
                for script in self.scripts {
                    context.eval::<(), _>(*script)?;
                }
                context.eval::<(), _>(self.bridge)?;
                let render: Function = context.globals().get(self.entry)?;
                render.call((input,))
            })()
            .catch(&context)
            .map_err(|error| {
                format!(
                    "{}: {}",
                    self.name,
                    error.to_string().chars().take(2048).collect::<String>()
                )
            })
        })?;
        if output.len() > super::MAX_SVG {
            return Err(format!(
                "{}: generated SVG exceeds the preview budget",
                self.name
            ));
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn calls_are_isolated_data_only_and_have_no_external_host() {
        let program = Program {
            name: "fixture",
            stack_bytes: 512 * 1024,
            scripts: &[],
            entry: "render",
            bridge: r#"globalThis.render = input => {
                if ([typeof require, typeof document, typeof fetch, typeof std, typeof os].some(x => x !== 'undefined')) throw Error('host installed');
                globalThis.counter = (globalThis.counter || 0) + 1;
                return counter + ':' + input;
            };"#,
        };
        assert_eq!(
            program.call("throw new Error('executed')").unwrap(),
            "1:throw new Error('executed')"
        );
        assert_eq!(program.call("next").unwrap(), "1:next");
    }
    #[test]
    fn interrupts_nonterminating_layout_and_bounds_generated_output() {
        let mut program = Program {
            name: "fixture",
            stack_bytes: 512 * 1024,
            scripts: &[],
            entry: "render",
            bridge: "globalThis.render = () => { while(true) {} };",
        };
        let started = Instant::now();
        assert!(program.call("").is_err());
        assert!(started.elapsed() < Duration::from_secs(6));
        program.bridge = "globalThis.render = () => 'x'.repeat(2097153);";
        assert!(program.call("").unwrap_err().contains("budget"));
    }
}
