globalThis.__renderVega = async input => {
    const request = JSON.parse(input);
    const warnings = [];
    const errors = [];
    let denied = false;
    const resourceError = 'Vega previews accept inline data only; linked data, images and URLs are disabled';
    const message = args => args.map(value => String(value)).join(' ').slice(0, 1024);
    const logger = {
        level(value) { return arguments.length ? this : vega.Warn; },
        warn(...args) { if (warnings.length < 16) warnings.push(message(args)); return this; },
        error(...args) { if (errors.length < 16) errors.push(message(args)); return this; },
        info() { return this; }, debug() { return this; }
    };
    const refuse = () => {
        denied = true;
        return Promise.reject(Error(resourceError));
    };
    const loader = {load: refuse, sanitize: refuse};
    const spec = request.lite ? vegaLite.compile(request.spec, {logger}).spec : request.spec;
    const view = new vega.View(vega.parse(spec), {renderer: 'none', loader, logger});
    try {
        const svg = await view.toSVG();
        if (denied) throw Error(resourceError);
        if (errors.length) throw Error(errors.join('\n'));
        return JSON.stringify({svg, warnings});
    } catch (error) {
        if (denied) throw Error(resourceError);
        throw error;
    } finally {
        view.finalize();
    }
};
