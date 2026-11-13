// SPDX-License-Identifier: MPL-2.0

// syscall/js.Value.String, with jsString delegated to the JS host exactly as
// Go's wasm_exec.js does. This is value marshaling; D2 algorithms stay in Rust.
function goValueString(value) {
  switch (typeof value) {
    case "string": return value;
    case "undefined": return "<undefined>";
    case "boolean": return `<boolean: ${String(value)}>`;
    case "number": return `<number: ${String(value)}>`;
    case "symbol": return "<symbol>";
    case "function": return "<function>";
    case "object": return value === null ? "<null>" : "<object>";
    default: throw new TypeError(`unsupported syscall/js value type: ${typeof value}`);
  }
}

function wrapGoStringArgument(method) {
  return function (...args) {
    // An explicitly passed undefined is a value. Only an omitted argument
    // takes the Rust helper's missing-argument branch.
    return args.length === 0 ? method() : method(goValueString(args[0]));
  };
}

module.exports = { wrapGoStringArgument };
