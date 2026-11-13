// QuickJS has no browser structuredClone. Vega-Lite clones configuration and
// normalized specifications: preserve undefined, non-finite numbers, Dates,
// Maps and Sets as well as JSON values. This is not a host/DOM binding.
if (typeof structuredClone === 'undefined') {
    globalThis.structuredClone = value => {
        const seen = new Map();
        // Leaf objects have intrinsic payloads rather than enumerable children.
        // Buffer views reuse the same memoized cloned buffer as sibling views.
        const copyLeaf = item => {
            if (item instanceof Date) return new Date(item.getTime());
            if (item instanceof RegExp) return new RegExp(item.source, item.flags);
            if (item instanceof ArrayBuffer) return item.slice(0);
            if (ArrayBuffer.isView(item)) {
                const buffer = copy(item.buffer);
                return item instanceof DataView
                    ? new DataView(buffer, item.byteOffset, item.byteLength)
                    : new item.constructor(buffer, item.byteOffset, item.length);
            }
        };
        const copy = item => {
            if (typeof item === 'function' || typeof item === 'symbol') {
                throw Error('Vega-Lite cannot clone executable values');
            }
            if (item === null || typeof item !== 'object') return item;
            if (seen.has(item)) return seen.get(item);
            let result = copyLeaf(item);
            if (result !== undefined) {
                seen.set(item, result);
                return result;
            }
            if (item instanceof Map) {
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
