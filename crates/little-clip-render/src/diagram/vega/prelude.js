// QuickJS has no browser structuredClone. Vega-Lite clones configuration and
// normalized specifications: preserve undefined, non-finite numbers, Dates,
// Maps and Sets as well as JSON values. This is not a host/DOM binding.
if (typeof structuredClone === 'undefined') {
    globalThis.structuredClone = value => {
        const seen = new Map();
        const copy = item => {
            if (typeof item === 'function' || typeof item === 'symbol') {
                throw Error('Vega-Lite cannot clone executable values');
            }
            if (item === null || typeof item !== 'object') return item;
            if (seen.has(item)) return seen.get(item);
            let result;
            if (item instanceof Date) result = new Date(item.getTime());
            else if (item instanceof RegExp) result = new RegExp(item.source, item.flags);
            else if (item instanceof ArrayBuffer) result = item.slice(0);
            else if (ArrayBuffer.isView(item)) {
                const buffer = copy(item.buffer);
                result = item instanceof DataView
                    ? new DataView(buffer, item.byteOffset, item.byteLength)
                    : new item.constructor(buffer, item.byteOffset, item.length);
            } else if (item instanceof Map) {
                result = new Map();
                seen.set(item, result);
                for (const [key, value] of item) result.set(copy(key), copy(value));
            } else if (item instanceof Set) {
                result = new Set();
                seen.set(item, result);
                for (const value of item) result.add(copy(value));
            } else {
                result = Array.isArray(item) ? new Array(item.length) : {};
                seen.set(item, result);
                for (const key of Object.keys(item)) {
                    Object.defineProperty(result, key, {
                        value: copy(item[key]), enumerable: true, writable: true, configurable: true
                    });
                }
            }
            seen.set(item, result);
            return result;
        };
        return copy(value);
    };
}
