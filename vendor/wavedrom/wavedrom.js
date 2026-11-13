"use strict";
(() => {
  var __getOwnPropNames = Object.getOwnPropertyNames;
  var __commonJS = (cb, mod) => function __require() {
    try {
      return mod || (0, cb[__getOwnPropNames(cb)[0]])((mod = { exports: {} }).exports, mod), mod.exports;
    } catch (e) {
      throw mod = 0, e;
    }
  };

  // package.json
  var require_package = __commonJS({
    "package.json"(exports2, module2) {
      module2.exports = {
        name: "wavedrom",
        version: "3.8.0",
        description: "Digital timing diagram in your browser",
        homepage: "http://wavedrom.com",
        author: "alex.drom@gmail.com",
        license: "MIT",
        repository: {
          type: "git",
          url: "https://github.com/wavedrom/wavedrom.git"
        },
        bugs: {
          url: "https://github.com/wavedrom/wavedrom/issues"
        },
        main: "./lib",
        bin: {
          wavedrom: "bin/cli.js"
        },
        exports: {
          ".": "./lib/index.js",
          "./package.json": "./package.json",
          "./skins/*": "./skins/*"
        },
        unpkg: "wavedrom.unpkg.min.js",
        jsdelivr: "wavedrom.unpkg.min.js",
        files: [
          "bin/cli.js",
          "wavedrom.js",
          "wavedrom.min.js",
          "wavedrom.unpkg.js",
          "wavedrom.unpkg.min.js",
          "LICENSE",
          "lib/**",
          "skins/**"
        ],
        scripts: {
          test: "npm run eslint && npm run c8",
          eslint: "eslint lib bin",
          c8: "c8 -r=lcov -r=text mocha test",
          dist: "esbuild ./lib/wave-drom.js --bundle --format=iife --outfile=wavedrom.js",
          "watch.dist": "esbuild ./lib/wave-drom.js --bundle --format=iife --outfile=wavedrom.js --watch",
          "dist.min": `node -e "const banner = require('child_process').execSync('node ./bin/header.js < /dev/null').toString(); require('child_process').execSync('esbuild ./lib/wave-drom.js --bundle --format=iife --minify --outfile=wavedrom.min.js --banner:js=\\"' + banner.trim() + '\\"')"`,
          unpkg: "node ./bin/unpkg.js",
          "unpkg.min": "node ./bin/unpkg.js --minify",
          prepare: "npm run test && npm run dist && npm run dist.min && npm run unpkg && npm run unpkg.min",
          clean: `node -e "const fs = require('fs'); ['wavedrom.js', 'coverage', '.nyc_output'].forEach(p => fs.rmSync(p, { recursive: true, force: true })); fs.readdirSync('.').forEach(f => { if (f.startsWith('wavedrom.') && f.endsWith('.js')) { fs.rmSync(f, { force: true }); } })"`,
          skins: `node -e "['default', 'narrow', 'dark', 'lowkey', 'narrower', 'narrowerer'].forEach(S => require('child_process').execSync('node bin/svg2js.js -i unpacked/skins/' + S + '.svg > skins/' + S + '.js'))"`
        },
        keywords: [
          "waveform",
          "verilog",
          "RTL"
        ],
        engines: {
          node: ">=20"
        },
        devDependencies: {
          "@drom/eslint-config": "1.0.0",
          c8: "12.0.0",
          chai: "6.3.0",
          esbuild: "0.28.2",
          eslint: "9.39.4",
          mocha: "12.0.3"
        },
        dependencies: {
          "bit-field": "^1.11.0",
          json5: "^2.2.3",
          logidrom: "^1.2.1",
          onml: "^2.3.0",
          tspan: "^0.4.0"
        }
      };
    }
  });

  // lib/eva.js
  var require_eva = __commonJS({
    "lib/eva.js"(exports, module) {
      "use strict";
      function erra(e) {
        console.log("Error in WaveJS: ", e);
        const msg = ["tspan", ["tspan", { class: "error h5" }, "Error: "], e.message];
        msg.textWidth = 1e3;
        return { signal: [{ name: msg }] };
      }
      function eva(id) {
        const TheTextBox = document.getElementById(id);
        let source;
        if (TheTextBox.type && TheTextBox.type === "textarea") {
          try {
            source = eval("(" + TheTextBox.value + ")");
          } catch (e) {
            return erra(e);
          }
        } else {
          try {
            source = eval("(" + TheTextBox.innerHTML + ")");
          } catch (e) {
            return erra(e);
          }
        }
        if (Object.prototype.toString.call(source) !== "[object Object]") {
          return erra({ message: '[Semantic]: The root has to be an Object: "{signal:[...]}"' });
        }
        if (source.signal) {
          if (!Array.isArray(source.signal)) {
            return erra({ message: '[Semantic]: "signal" object has to be an Array "signal:[]"' });
          }
        } else if (source.assign) {
          if (!Array.isArray(source.assign)) {
            return erra({ message: '[Semantic]: "assign" object hasto be an Array "assign:[]"' });
          }
        } else if (source.reg) {
        } else {
          return erra({ message: '[Semantic]: "signal:[...]" or "assign:[...]" property is missing inside the root Object' });
        }
        return source;
      }
      module.exports = eva;
    }
  });

  // node_modules/onml/stringify.js
  var require_stringify = __commonJS({
    "node_modules/onml/stringify.js"(exports2, module2) {
      "use strict";
      var isObject = (o) => o && Object.prototype.toString.call(o) === "[object Object]";
      function indenter(indentation) {
        if (!(indentation > 0)) {
          return (txt) => txt;
        }
        var space = " ".repeat(indentation);
        return (txt) => {
          if (typeof txt !== "string") {
            return txt;
          }
          const arr = txt.split("\n");
          if (arr.length === 1) {
            return space + txt;
          }
          return arr.map((e) => e.trim() === "" ? e : space + e).join("\n");
        };
      }
      var clean = (txt) => txt.split("\n").filter((e) => e.trim() !== "").join("\n");
      function stringify(a, indentation) {
        const cr = indentation > 0 ? "\n" : "";
        const indent = indenter(indentation);
        function rec(a2) {
          let body = "";
          let isFlat = true;
          let res;
          const isEmpty = a2.some((e, i, arr) => {
            if (i === 0) {
              res = "<" + e;
              return arr.length === 1;
            }
            if (i === 1) {
              if (isObject(e)) {
                Object.keys(e).map((key) => {
                  let val = e[key];
                  if (Array.isArray(val)) {
                    val = val.join(" ");
                  }
                  res += " " + key + '="' + val + '"';
                });
                if (arr.length === 2) {
                  return true;
                }
                res += ">";
                return;
              }
              res += ">";
            }
            if (!e && e !== 0 && e !== "") {
              return;
            }
            switch (typeof e) {
              case "string":
              case "number":
              case "boolean":
              case "undefined":
                body += e + cr;
                return;
            }
            isFlat = false;
            body += rec(e);
          });
          if (isEmpty) {
            return res + "/>" + cr;
          }
          return isFlat ? res + clean(body) + "</" + a2[0] + ">" + cr : res + cr + indent(body) + "</" + a2[0] + ">" + cr;
        }
        return rec(a);
      }
      module2.exports = stringify;
    }
  });

  // node_modules/logidrom/lib/tree-utils.js
  var require_tree_utils = __commonJS({
    "node_modules/logidrom/lib/tree-utils.js"(exports2) {
      "use strict";
      var isAttrs = (v) => v !== null && typeof v === "object" && !Array.isArray(v) && !Object.prototype.hasOwnProperty.call(v, "x");
      var firstChildIdx = (tree) => isAttrs(tree[1]) ? 2 : 1;
      var getAttrs = (tree) => isAttrs(tree[1]) ? tree[1] : null;
      var isLeafCone = (tree) => Array.isArray(tree) && tree.length === firstChildIdx(tree);
      var getWidth = (node) => {
        if (!Array.isArray(node)) return 1;
        const attrs = getAttrs(node);
        return attrs && attrs.width || 1;
      };
      var widther = (w) => w === 0 ? "zeroer" : w === 1 ? "scalar" : "vector";
      var nameOf = (node) => typeof node === "string" ? node : node.name;
      var pinLabel = (op, attrs) => {
        if (op !== "pin" && op !== "pout" && op !== "pinout") return null;
        if (!attrs) return null;
        const ins = attrs.instance || "";
        const pin = attrs.pin || "";
        return ins + "." + pin;
      };
      var pinLabels = (op, attrs) => {
        if (!attrs) return { instance: "", pin: "" };
        return { instance: attrs.instance || "", pin: attrs.pin || "" };
      };
      var leafDisplay = (branch) => {
        if (!Array.isArray(branch)) return nameOf(branch);
        const attrs = getAttrs(branch);
        const name = nameOf(branch[0]);
        const pl = pinLabel(name, attrs);
        if (pl !== null) return pl;
        return name;
      };
      var outDisplay = (assignTree) => {
        const attrs = getAttrs(assignTree);
        const op = nameOf(assignTree[0]);
        const pl = pinLabel(op, attrs);
        if (pl !== null) return pl;
        const start = firstChildIdx(assignTree);
        const branch = assignTree[start];
        const visible = Array.isArray(branch) ? nameOf(branch[0]) : nameOf(branch);
        return visible || (attrs && attrs.label || "");
      };
      var outTooltip = (assignTree) => {
        const attrs = getAttrs(assignTree);
        const op = nameOf(assignTree[0]);
        const pl = pinLabel(op, attrs);
        if (pl !== null) return pl;
        if (attrs && attrs.label) return attrs.label;
        const start = firstChildIdx(assignTree);
        const branch = assignTree[start];
        return Array.isArray(branch) ? nameOf(branch[0]) : nameOf(branch);
      };
      var inlineDisplay = (assignTree) => {
        const attrs = getAttrs(assignTree);
        const op = nameOf(assignTree[0]);
        const pl = pinLabel(op, attrs);
        if (pl !== null) return pl;
        const start = firstChildIdx(assignTree);
        const nameBranch = assignTree[start];
        const node = Array.isArray(nameBranch) ? nameBranch[0] : nameBranch;
        return node && node.name || "";
      };
      var isPinOp = (op) => op === "pin" || op === "pout" || op === "pinout";
      exports2.isAttrs = isAttrs;
      exports2.firstChildIdx = firstChildIdx;
      exports2.getAttrs = getAttrs;
      exports2.isLeafCone = isLeafCone;
      exports2.getWidth = getWidth;
      exports2.widther = widther;
      exports2.leafDisplay = leafDisplay;
      exports2.outDisplay = outDisplay;
      exports2.outTooltip = outTooltip;
      exports2.inlineDisplay = inlineDisplay;
      exports2.isPinOp = isPinOp;
      exports2.pinLabel = pinLabel;
      exports2.pinLabels = pinLabels;
    }
  });

  // node_modules/logidrom/lib/render.js
  var require_render = __commonJS({
    "node_modules/logidrom/lib/render.js"(exports2, module2) {
      "use strict";
      var { firstChildIdx } = require_tree_utils();
      function render(tree, state) {
        state.xmax = Math.max(state.xmax, state.x);
        const y = state.y;
        const start = firstChildIdx(tree);
        const ilen = tree.length;
        if (ilen === start) {
          tree[0] = { name: tree[0], x: state.x, y: state.y };
          state.y += 2;
          state.x--;
          return state;
        }
        const isAssign = tree[0] === "=" && ilen > start + 1;
        const childStart = isAssign ? start + 1 : start;
        const childYs = [];
        for (let i = childStart; i < ilen; i++) {
          const branch = tree[i];
          if (Array.isArray(branch)) {
            state = render(branch, {
              x: state.x + 1,
              y: state.y,
              xmax: state.xmax
            });
            const node = branch[0];
            if (node && typeof node === "object" && typeof node.y === "number") {
              childYs.push(node.y);
            }
          } else {
            const node = {
              name: branch,
              x: state.x + 1,
              y: state.y
            };
            tree[i] = node;
            state.xmax = Math.max(state.xmax, state.x + 1);
            childYs.push(node.y);
            state.y += 2;
          }
        }
        let gateY;
        if (childYs.length === 0) {
          gateY = y;
        } else {
          childYs.sort((a, b) => a - b);
          const mid = Math.floor(childYs.length / 2);
          if (childYs.length % 2 === 1) {
            gateY = childYs[mid];
          } else {
            gateY = Math.round((childYs[mid - 1] + childYs[mid]) / 2);
          }
        }
        tree[0] = { name: tree[0], x: state.x, y: gateY };
        if (isAssign) {
          const nameBranch = tree[start];
          if (Array.isArray(nameBranch)) {
            nameBranch[0] = { name: nameBranch[0], x: state.x, y: gateY };
          } else {
            tree[start] = { name: nameBranch, x: state.x, y: gateY };
          }
        }
        state.x--;
        return state;
      }
      module2.exports = render;
    }
  });

  // node_modules/tspan/lib/parse.js
  var require_parse = __commonJS({
    "node_modules/tspan/lib/parse.js"(exports2, module2) {
      "use strict";
      var escapeMap = {
        "&": "&amp;",
        '"': "&quot;",
        "<": "&lt;",
        ">": "&gt;"
      };
      function xscape(val) {
        if (typeof val !== "string") {
          return val;
        }
        return val.replace(
          /([&"<>])/g,
          function(_, e) {
            return escapeMap[e];
          }
        );
      }
      var token = /<o>|<ins>|<s>|<sub>|<sup>|<b>|<i>|<tt>|<\/o>|<\/ins>|<\/s>|<\/sub>|<\/sup>|<\/b>|<\/i>|<\/tt>/;
      function update(s, cmd) {
        if (cmd.add) {
          cmd.add.split(";").forEach(function(e) {
            var arr = e.split(" ");
            s[arr[0]][arr[1]] = true;
          });
        }
        if (cmd.del) {
          cmd.del.split(";").forEach(function(e) {
            var arr = e.split(" ");
            delete s[arr[0]][arr[1]];
          });
        }
      }
      var trans = {
        "<o>": { add: "text-decoration overline" },
        "</o>": { del: "text-decoration overline" },
        "<ins>": { add: "text-decoration underline" },
        "</ins>": { del: "text-decoration underline" },
        "<s>": { add: "text-decoration line-through" },
        "</s>": { del: "text-decoration line-through" },
        "<b>": { add: "font-weight bold" },
        "</b>": { del: "font-weight bold" },
        "<i>": { add: "font-style italic" },
        "</i>": { del: "font-style italic" },
        "<sub>": { add: "baseline-shift sub;font-size .7em" },
        "</sub>": { del: "baseline-shift sub;font-size .7em" },
        "<sup>": { add: "baseline-shift super;font-size .7em" },
        "</sup>": { del: "baseline-shift super;font-size .7em" },
        "<tt>": { add: "font-family monospace" },
        "</tt>": { del: "font-family monospace" }
      };
      function dump(s) {
        return Object.keys(s).reduce(function(pre, cur) {
          var keys = Object.keys(s[cur]);
          if (keys.length > 0) {
            pre[cur] = keys.join(" ");
          }
          return pre;
        }, {});
      }
      function parse(str) {
        var state, res, i, m, a;
        if (str === void 0) {
          return [];
        }
        if (typeof str === "number") {
          return [str + ""];
        }
        if (typeof str !== "string") {
          return [str];
        }
        res = [];
        state = {
          "text-decoration": {},
          "font-weight": {},
          "font-style": {},
          "baseline-shift": {},
          "font-size": {},
          "font-family": {}
        };
        while (true) {
          i = str.search(token);
          if (i === -1) {
            res.push(["tspan", dump(state), xscape(str)]);
            return res;
          }
          if (i > 0) {
            a = str.slice(0, i);
            res.push(["tspan", dump(state), xscape(a)]);
          }
          m = str.match(token)[0];
          update(state, trans[m]);
          str = str.slice(i + m.length);
          if (str.length === 0) {
            return res;
          }
        }
      }
      module2.exports = parse;
    }
  });

  // node_modules/tspan/lib/reparse.js
  var require_reparse = __commonJS({
    "node_modules/tspan/lib/reparse.js"(exports2, module2) {
      "use strict";
      var parse = require_parse();
      function deDash(str) {
        var m = str.match(/(\w+)-(\w)(\w+)/);
        if (m === null) {
          return str;
        }
        var newStr = m[1] + m[2].toUpperCase() + m[3];
        return newStr;
      }
      function reparse(React) {
        var $ = React.createElement;
        function reTspan(e, i) {
          var tag = e[0];
          var attr = e[1];
          var newAttr = Object.keys(attr).reduce(function(res, key) {
            var newKey = deDash(key);
            res[newKey] = attr[key];
            return res;
          }, {});
          var body = e[2];
          newAttr.key = i;
          return $(tag, newAttr, body);
        }
        return function(str) {
          return parse(str).map(reTspan);
        };
      }
      module2.exports = reparse;
    }
  });

  // node_modules/tspan/lib/index.js
  var require_lib = __commonJS({
    "node_modules/tspan/lib/index.js"(exports2) {
      "use strict";
      var parse = require_parse();
      var reparse = require_reparse();
      exports2.parse = parse;
      exports2.reparse = reparse;
    }
  });

  // node_modules/logidrom/lib/font-metrics.js
  var require_font_metrics = __commonJS({
    "node_modules/logidrom/lib/font-metrics.js"(exports2) {
      "use strict";
      var CHAR_WIDTH_PX = 7.23;
      var getLabelWidth = (s, fontWidth) => {
        const charWidth = fontWidth !== void 0 ? fontWidth : CHAR_WIDTH_PX;
        return Math.ceil((String(s).length + 0.3) * charWidth / 8) * 8;
      };
      exports2.CHAR_WIDTH_PX = CHAR_WIDTH_PX;
      exports2.getLabelWidth = getLabelWidth;
    }
  });

  // node_modules/logidrom/lib/draw_body.js
  var require_draw_body = __commonJS({
    "node_modules/logidrom/lib/draw_body.js"(exports2, module2) {
      "use strict";
      var tspan = require_lib();
      var { getLabelWidth } = require_font_metrics();
      var circle = [
        "m",
        -6,
        -0,
        "a",
        3,
        3,
        0,
        1,
        1,
        6,
        0,
        "a",
        3,
        3,
        0,
        1,
        1,
        -6,
        0
      ];
      var buf1 = ["m", -12, -6, 12, 6, -12, 6, "z"];
      var and1 = [
        // reduction AND with 1 input
        "m",
        -16,
        -6,
        "h",
        6,
        "a",
        6,
        6,
        0,
        1,
        1,
        0,
        12,
        "h",
        -6,
        "z",
        "m",
        12,
        6,
        "h",
        4
      ];
      var or1 = [
        // reduction OR with 1 input
        "m",
        -17,
        6,
        "a",
        12,
        12,
        0,
        0,
        0,
        0,
        -12,
        "a",
        12,
        12,
        0,
        0,
        1,
        13,
        6,
        "a",
        12,
        12,
        0,
        0,
        1,
        -13,
        6,
        "z",
        "m",
        13,
        -6,
        "h",
        4
      ];
      var xor1 = [
        // reduction XOR with 1 input
        "m",
        -12,
        6,
        "a",
        12,
        12,
        0,
        0,
        0,
        0,
        -12,
        "a",
        12,
        12,
        0,
        0,
        1,
        9,
        6,
        "a",
        12,
        12,
        0,
        0,
        1,
        -9,
        6,
        "z",
        "m",
        -4,
        0,
        "a",
        12,
        12,
        0,
        0,
        0,
        2,
        -6,
        // second left
        "m",
        0,
        0,
        "a",
        12,
        12,
        0,
        0,
        0,
        -2,
        -6,
        "m",
        13,
        6,
        "h",
        3
      ];
      var and2 = [
        // AND gate with >1 inputs
        "m",
        -16,
        -12,
        "h",
        4,
        "a",
        12,
        12,
        0,
        1,
        1,
        0,
        24,
        "h",
        -4,
        "z"
      ];
      var or2 = [
        // OR gate with >1 inputs
        "m",
        -16,
        12,
        "a",
        28,
        28,
        0,
        0,
        0,
        0,
        -24,
        // left
        "a",
        18,
        18,
        0,
        0,
        1,
        16,
        12,
        // top
        "a",
        18,
        18,
        0,
        0,
        1,
        -16,
        12,
        // bottom
        "z"
      ];
      var xor2 = [
        // XOR gate with >1 inputs
        "m",
        -12,
        12,
        "a",
        28,
        28,
        0,
        0,
        0,
        0,
        -24,
        // left
        "a",
        18,
        18,
        0,
        0,
        1,
        12,
        12,
        // top
        "a",
        18,
        18,
        0,
        0,
        1,
        -12,
        12,
        // bottom
        "z",
        "m",
        -4,
        0,
        "a",
        28,
        28,
        0,
        0,
        0,
        3,
        -12,
        // second left
        "m",
        0,
        0,
        "a",
        28,
        28,
        0,
        0,
        0,
        -3,
        -12
      ];
      var circle2 = [
        "m",
        -10,
        -10,
        "a",
        10,
        10,
        0,
        1,
        1,
        0,
        20,
        "a",
        10,
        10,
        0,
        1,
        1,
        0,
        -20
      ];
      var gates = {
        "buf1": { w: 12, d: buf1 },
        "~": { w: 18, d: [...circle, ...buf1] },
        "&": { w: 16, d: and2 },
        "~&": { w: 22, d: [...circle, ...and2] },
        "&1": { w: 16, d: and1 },
        "~&1": { w: 22, d: [...circle, ...and1] },
        "|": { w: 16, d: or2 },
        "~|": { w: 22, d: [...circle, ...or2] },
        "|1": { w: 16, d: or1 },
        "~|1": { w: 22, d: [...circle, ...or1] },
        "^": { w: 16, d: xor2 },
        "~^": { w: 22, d: [...circle, ...xor2] },
        "^1": { w: 16, d: xor1 },
        "~^1": { w: 22, d: [...circle, ...xor1] },
        "+": { w: 16, d: ["m", -10, 5, 0, -10, "m", -5, 5, 10, 0, "m", 5, 0, ...circle2] },
        "*": { w: 16, d: ["m", -6, 4, -8, -8, "m", 0, 8, 8, -8, "m", 6, 4, ...circle2] },
        "-": { w: 16, d: ["m", -5, 0, -10, 0, "m", 15, 0, ...circle2] },
        "/": { w: 16, d: ["m", -6, -4, -8, 8, "m", 14, -4, ...circle2] },
        "%": { w: 16, d: [
          "m",
          -6,
          -4,
          -8,
          8,
          "m",
          1,
          -5,
          "a",
          2,
          2,
          0,
          1,
          1,
          0,
          -4,
          "a",
          2,
          2,
          0,
          1,
          1,
          0,
          4,
          // top left
          "m",
          6,
          2,
          "a",
          2,
          2,
          0,
          1,
          1,
          0,
          4,
          "a",
          2,
          2,
          0,
          1,
          1,
          0,
          -4,
          // bottom right
          "m",
          7,
          -1,
          ...circle2
        ] }
      };
      var aliasGates = {
        add: "+",
        mul: "*",
        sub: "-",
        and: "&",
        or: "|",
        xor: "^",
        andr: "&",
        orr: "|",
        xorr: "^",
        input: "buf1"
      };
      Object.keys(aliasGates).reduce((res, key) => {
        res[key] = gates[aliasGates[key]];
        return res;
      }, gates);
      var gater1 = {
        is: (type) => gates[type] !== void 0,
        render: (type) => ["path", { w: gates[type].w, h: 16, class: "gate", d: gates[type].d }]
      };
      var iec = {
        eq: "==",
        ne: "!=",
        slt: "<",
        sle: "<=",
        sgt: ">",
        sge: ">=",
        ult: "<",
        ule: "<=",
        ugt: ">",
        uge: ">=",
        BUF: 1,
        INV: 1,
        AND: "&",
        NAND: "&",
        OR: "\u22651",
        NOR: "\u22651",
        XOR: "=1",
        XNOR: "=1",
        box: "",
        CONCAT: "}",
        case: "C",
        casez: "Z",
        casex: "X"
      };
      var circled = { INV: 1, NAND: 1, NOR: 1, XNOR: 1 };
      var gater2 = {
        is: (type) => iec[type] !== void 0,
        render: (type, ymin, ymax) => {
          if (ymin === ymax) {
            ymin = -4;
            ymax = 4;
          }
          return [
            "g",
            { w: 16, h: ymax - ymin + 6 },
            ["path", {
              class: "gate",
              d: ["m", -16, ymin - 3, 16, 0, 0, ymax - ymin + 6, -16, 0, "z", ...circled[type] ? circle : []]
            }],
            ["text", { x: -14, y: 4, class: "wirename" }, ...tspan.parse(iec[type])]
          ];
        }
      };
      var isSlice = (type) => typeof type === "string" && type[0] === "[";
      function drawBody(type, ymin, ymax, fontWidth, attrs) {
        if (gater1.is(type)) {
          return gater1.render(type);
        }
        if (gater2.is(type)) {
          return gater2.render(type, ymin, ymax);
        }
        if (isSlice(type)) {
          const bodyW2 = getLabelWidth(type, fontWidth) + 8;
          return [
            "text",
            { w: bodyW2, h: 16, x: -bodyW2 / 2, y: 4, class: "slicelabel" },
            ...tspan.parse(type)
          ];
        }
        if (type === "MUX") {
          return [
            "g",
            { w: 12, h: 40, o: -8 },
            ["path", { class: "gate", d: [
              "m",
              -12,
              -24,
              12,
              6,
              0,
              20,
              -12,
              6,
              "z",
              "m",
              0,
              40,
              7,
              0,
              "m",
              0,
              0,
              0,
              -11
            ] }],
            ["text", { x: -7, y: -12, class: "bodylabel" }, "0"],
            ["text", { x: -7, y: 2, class: "bodylabel" }, "1"]
          ];
        }
        {
          const m = type.match(/^ff(?<negedge>n)?(?<enable>e)?((?<syncReset>[cp])?(?<syncResetPolarity>n)?)?(?<asyncReset>[rs])?((?<asyncResetPolarity>n)?)?$/);
          if (m) {
            const { negedge, enable, syncReset, syncResetPolarity, asyncReset, asyncResetPolarity } = m.groups;
            const hasNegedge = negedge === "n";
            const hasEnable = enable === "e";
            const hasSyncReset = syncReset !== void 0;
            const hasSyncSet = syncReset === "p";
            const hasSyncResetPolarity = syncResetPolarity === "n";
            const hasAsyncReset = asyncReset !== void 0;
            const hasAsyncSet = asyncReset === "s";
            const hasAsyncResetPolarity = asyncResetPolarity === "n";
            let h = 32;
            if (hasEnable) h += 16;
            if (hasSyncReset) h += 16;
            if (hasSyncSet) h += 16;
            if (hasAsyncReset) h += 16;
            if (hasAsyncSet) h += 16;
            let o = -(h / 2 - 8);
            return [
              "g",
              { w: 32, h, o },
              ["path", { class: "dff", d: [
                "m",
                -32,
                o - 7,
                "h",
                32,
                "v",
                30 + (hasEnable ? 16 : 0) + (hasSyncReset ? 16 : 0) + (hasSyncSet ? 16 : 0),
                "h",
                -32,
                "z",
                "m",
                0,
                19,
                6,
                4,
                -6,
                4,
                // wedge '>'
                ...hasNegedge ? ["m", 0, -4, "a", 3, 3, 0, 1, 1, -6, 0, "a", 3, 3, 0, 1, 1, 6, 0, "z", "m", 0, 4] : [],
                // negedge clock bubble
                ...hasEnable ? ["m", 0, 16] : [],
                // extra offset for enable
                ...hasSyncReset ? [
                  "m",
                  0,
                  16,
                  ...hasSyncResetPolarity ? ["m", 0, -4, "a", 3, 3, 0, 1, 1, -6, 0, "a", 3, 3, 0, 1, 1, 6, 0, "z", "m", 0, 4] : []
                ] : [],
                // extra offset for sync reset
                ...hasAsyncReset ? [
                  // async reset/set wire extension
                  "m",
                  0,
                  12 + (hasSyncSet ? 16 : 0),
                  "h",
                  16,
                  "m",
                  0,
                  0,
                  "v",
                  ...hasAsyncResetPolarity ? [-3, "m", -16, -9] : [-9, "m", -16, -1]
                ] : [],
                ...hasAsyncResetPolarity ? ["m", 16, 3, "a", 3, 3, 0, 1, 1, 0, 6, "a", 3, 3, 0, 1, 1, 0, -6, "z"] : []
                // low-active async reset/set bubble
              ] }],
              // FF label
              ["text", { x: -16, y: o + 4, class: "bodylabel" }, "DFF"],
              // enable label
              ...hasEnable ? [["text", { x: -28, y: o + 36, class: "bodylabel" }, "E"]] : [],
              // sync reset label
              ...hasSyncReset ? [[
                "text",
                { x: -28, y: o + 36 + (hasEnable ? 16 : 0), class: "bodylabel" },
                hasSyncResetPolarity ? ["tspan", { "text-decoration": "overline" }, hasSyncSet ? "P" : "C"] : hasSyncSet ? "P" : "C"
              ]] : [],
              // sync preset value label
              ...hasSyncSet ? [["text", { x: -28, y: o + 36 + (hasEnable ? 16 : 0) + 16, class: "bodylabel" }, "V"]] : [],
              // async reset label
              ...hasAsyncReset ? [[
                "text",
                { x: -16, y: o + 16 + 5 + (hasEnable ? 16 : 0) + (hasSyncReset ? 16 : 0) + (hasSyncSet ? 16 : 0), class: "bodylabel" },
                hasAsyncResetPolarity ? ["tspan", { "text-decoration": "overline" }, hasAsyncSet ? "S" : "R"] : hasAsyncSet ? "S" : "R"
              ]] : [],
              // async set value label
              ...hasAsyncSet ? [["text", { x: -16, y: o + 36 + 16 + (hasEnable ? 16 : 0) + (hasSyncReset ? 16 : 0) + (hasSyncSet ? 16 : 0), class: "bodylabel" }, "INI"]] : []
            ];
          }
        }
        const label = attrs && attrs.imm ? type + " " + attrs.imm : type;
        const bodyW = getLabelWidth(label, fontWidth);
        return [
          "g",
          { w: bodyW, h: 16 },
          ["rect", { class: "gate", x: -bodyW, y: -8, width: bodyW, height: 16 }],
          ["text", { x: -bodyW / 2, y: 4, class: "bodylabel" }, ...tspan.parse(label)]
        ];
      }
      module2.exports = drawBody;
      module2.exports.isShape = (type) => gater1.is(type);
    }
  });

  // node_modules/logidrom/lib/draw_gate.js
  var require_draw_gate = __commonJS({
    "node_modules/logidrom/lib/draw_gate.js"(exports2, module2) {
      "use strict";
      var tspan = require_lib();
      var drawBody = require_draw_body();
      var { widther } = require_tree_utils();
      var INPUT_SPACING = 16;
      var SHAPE_BODY_HALF_H = 10;
      function drawGate(spec, attrs) {
        const nInputs = spec.length - 2;
        const [gateX, gateY] = spec[1];
        const ret = ["g"];
        const isShapeGate = drawBody.isShape(spec[0]);
        const targetYs = Array.from({ length: nInputs }, () => 0);
        if (nInputs) {
          if (isShapeGate && nInputs > 2) {
            for (let i = 0; i < nInputs; i++) {
              targetYs[i] = spec[2 + i][1];
            }
          } else {
            const baseYs = [];
            for (let r = 0; r < nInputs; r++) {
              baseYs.push(gateY + (r - (nInputs - 1) / 2) * INPUT_SPACING);
            }
            const order = [];
            for (let i = 0; i < nInputs; i++) order.push(i);
            order.sort((a, b) => {
              const ca = spec[2 + a][1];
              const cb = spec[2 + b][1];
              return ca - cb;
            });
            for (let rank = 0; rank < nInputs; rank++) {
              const idx = order[rank];
              targetYs[idx] = baseYs[rank];
            }
          }
        }
        const ymin = nInputs ? Math.min.apply(null, targetYs) : gateY;
        const ymax = nInputs ? Math.max.apply(null, targetYs) : gateY;
        const body = drawBody(spec[0], ymin - gateY, ymax - gateY, void 0, attrs);
        const bodyHalfL = body[1].w || 0;
        if (nInputs <= 2 || isShapeGate) {
          for (let i = 0; i < nInputs; i++) {
            const [cx, cy, cw] = spec[2 + i];
            const ty = targetYs[i];
            const runLen = gateX - cx - bodyHalfL;
            let d;
            if (cy === ty) {
              d = "M" + cx + "," + cy + " h" + runLen;
            } else {
              const half = runLen / 2;
              d = "M" + cx + "," + cy + " h" + half + " v" + (ty - cy) + " h" + half;
            }
            const path = ["path", { d, class: ["wire", widther(cw)] }];
            if (cw > 1) path.push(["title", cw + " bits"]);
            ret.push(path);
          }
        } else {
          const inputs = [];
          for (let i2 = 0; i2 < nInputs; i2++) {
            const [cx, cy, cw] = spec[2 + i2];
            const ty = targetYs[i2];
            inputs.push({ cx, cy, cw, ty });
          }
          const backBaseX = gateX - bodyHalfL;
          let i = 0;
          for (; i < nInputs; i++) {
            const inp = inputs[i];
            const d = ["M", inp.cx, inp.cy];
            const deltaY = inp.ty - inp.cy;
            if (deltaY < 0) break;
            if (deltaY === 0) {
              d.push("H", backBaseX);
            } else {
              const x = backBaseX - (i + 1) * 8;
              d.push("H", x, "V", inp.ty, "H", backBaseX);
            }
            const path = ["path", { d, class: ["wire", widther(inp.cw)] }];
            if (inp.cw > 1) path.push(["title", inp.cw + " bits"]);
            ret.push(path);
          }
          for (let j = nInputs - 1; j >= i; j--) {
            const inp = inputs[j];
            const d = ["M", inp.cx, inp.cy];
            const channelIdx = nInputs - 1 - j;
            const x = backBaseX - (channelIdx + 1) * 8;
            d.push("H", x, "V", inp.ty, "H", backBaseX);
            const path = ["path", { d, class: ["wire", widther(inp.cw)] }];
            if (inp.cw > 1) path.push(["title", inp.cw + " bits"]);
            ret.push(path);
          }
        }
        if (nInputs > 2 && isShapeGate) {
          let bodyHalfH = SHAPE_BODY_HALF_H;
          if (Array.isArray(body) && body.length > 1 && body[1] && typeof body[1] === "object" && !Array.isArray(body[1])) {
            if (typeof body[1].h === "number") {
              bodyHalfH = Math.round(body[1].h / 2);
            }
          }
          bodyHalfH = Math.round(bodyHalfH / 8) * 8;
          const bodyTop = gateY - bodyHalfH - 4;
          const bodyBottom = gateY + bodyHalfH + 4;
          const backX = gateX - bodyHalfL;
          let topY = Infinity;
          let bottomY = -Infinity;
          for (let i = 0; i < nInputs; i++) {
            const ty = targetYs[i];
            if (ty < bodyTop && ty < topY) topY = ty;
            if (ty > bodyBottom && ty > bottomY) bottomY = ty;
          }
          const parts = [];
          if (topY < Infinity) {
            parts.push("M" + backX + "," + topY + " V" + bodyTop);
          }
          if (bottomY > -Infinity) {
            parts.push("M" + backX + "," + bodyBottom + " V" + bottomY);
          }
          if (parts.length) {
            ret.push(["path", { class: "gate", d: parts.join(" ") }]);
          }
        }
        ret.push([
          "g",
          { transform: "translate(" + gateX + "," + gateY + ")" },
          ["title", ...tspan.parse(spec[0])],
          body
        ]);
        return ret;
      }
      module2.exports = drawGate;
    }
  });

  // node_modules/logidrom/lib/draw_boxes.js
  var require_draw_boxes = __commonJS({
    "node_modules/logidrom/lib/draw_boxes.js"(exports2, module2) {
      "use strict";
      var tspan = require_lib();
      var drawGate = require_draw_gate();
      var drawBody = require_draw_body();
      var { getLabelWidth, CHAR_WIDTH_PX } = require_font_metrics();
      var { firstChildIdx, getAttrs, getWidth, widther, leafDisplay, outDisplay, outTooltip, inlineDisplay, isPinOp, pinLabel, pinLabels } = require_tree_utils();
      var LEAF_PAD_X = 4;
      var LEAF_HALF_H = 8;
      var INLINE_MIN_BOX_W = 32;
      var OUT_MIN_BOX_W = 32;
      var MIN_PASSTHRU_PX = 48;
      var textWidth = (s, fontWidth) => Math.ceil(String(s || "").length * (fontWidth || CHAR_WIDTH_PX));
      function portBoxWidth(gwInst, gwPin) {
        return gwInst + gwPin + 24;
      }
      function dirInBoxWidth(gw) {
        return gw + 14;
      }
      function dirOutBoxWidth(gw) {
        return gw + 16;
      }
      function pinPortPathD(gwPin) {
        return [
          "m",
          -gwPin - 14,
          -8,
          "l",
          6,
          8,
          "l",
          -6,
          8,
          "h",
          gwPin + 14,
          "v",
          -16,
          "z"
        ];
      }
      function pinBindPathD(gwInst, gwPin) {
        return [
          "m",
          -gwPin - 16,
          -8,
          "l",
          6,
          8,
          "l",
          -6,
          8,
          "h",
          -gwInst,
          "a",
          8,
          8,
          0,
          1,
          1,
          0,
          -16,
          "z"
        ];
      }
      function poutBindPathD(gwInst) {
        return [
          "m",
          -gwInst - 14,
          -8,
          "l",
          6,
          8,
          "l",
          -6,
          8,
          "h",
          gwInst + 6,
          "a",
          8,
          8,
          0,
          1,
          0,
          0,
          -16,
          "z"
        ];
      }
      function poutPortPathD(gwInst, gwPin) {
        return [
          "m",
          -gwInst - 16,
          -8,
          "l",
          6,
          8,
          "l",
          -6,
          8,
          "h",
          -gwPin - 8,
          "v",
          -16,
          "z"
        ];
      }
      function dirInPathD(gw) {
        return [
          "m",
          -gw - 14,
          -8,
          "l",
          6,
          8,
          "l",
          -6,
          8,
          "h",
          gw + 14,
          "v",
          -16,
          "z"
        ];
      }
      function dirOutPathD(gw) {
        return [
          "m",
          -6,
          8,
          "l",
          6,
          -8,
          "l",
          -6,
          -8,
          "h",
          -gw - 10,
          "v",
          16,
          "z"
        ];
      }
      function applyNavAttrs(groupAttrs, attrs) {
        if (!attrs) return;
        if (attrs.nodeId || attrs.siteKey) {
          groupAttrs.class = (groupAttrs.class ? groupAttrs.class + " " : "") + "rtl-node";
          if (attrs.nodeId) groupAttrs["data-node-id"] = attrs.nodeId;
          if (attrs.siteKey) groupAttrs["data-site-key"] = attrs.siteKey;
          if (attrs.module) groupAttrs["data-module"] = attrs.module;
          if (attrs.name) groupAttrs["data-name"] = attrs.name;
          if (attrs.loc) groupAttrs["data-loc"] = attrs.loc;
        }
      }
      function drawPortBox(tree, fx, fy, fontWidth) {
        const op = tree[0].name;
        const attrs = getAttrs(tree) || {};
        const labels = pinLabels(op, attrs);
        const gwInst = textWidth(labels.instance, fontWidth);
        const gwPin = textWidth(labels.pin, fontWidth);
        const tooltip = pinLabel(op, attrs) || "";
        const isPout = op === "pout";
        const portD = isPout ? poutPortPathD(gwInst, gwPin) : pinPortPathD(gwPin);
        const bindD = isPout ? poutBindPathD(gwInst) : pinBindPathD(gwInst, gwPin);
        let xLabelInst, xLabelPin;
        if (isPout) {
          xLabelInst = -Math.round((gwInst + 10) / 2);
          xLabelPin = -Math.round((2 * gwInst + gwPin + 40) / 2);
        } else {
          xLabelPin = -Math.round((gwPin + 10) / 2);
          xLabelInst = -Math.round((2 * gwPin + gwInst + 32) / 2);
        }
        const groupAttrs = { transform: "translate(" + fx + "," + fy + ")" };
        applyNavAttrs(groupAttrs, attrs);
        return [
          "g",
          groupAttrs,
          ["title", ...tspan.parse(tooltip)],
          ["path", { class: "port", d: portD }],
          ["path", { class: "bind", d: bindD }],
          ["text", { x: xLabelInst, y: 4, class: "bodylabel" }, ...tspan.parse(labels.instance)],
          ["text", { x: xLabelPin, y: 4, class: "bodylabel" }, ...tspan.parse(labels.pin)]
        ];
      }
      function drawDirInBox(label, attrs, fx, fy, fontWidth) {
        const gw = textWidth(label, fontWidth);
        const d = dirInPathD(gw);
        const xLabel = -Math.round((gw + 10) / 2);
        const groupAttrs = { transform: "translate(" + fx + "," + fy + ")" };
        applyNavAttrs(groupAttrs, attrs);
        return [
          "g",
          groupAttrs,
          ["title", ...tspan.parse(label)],
          ["path", { class: "port", d }],
          ["text", { x: xLabel, y: 4, class: "bodylabel" }, ...tspan.parse(label)]
        ];
      }
      function drawDirOutBox(label, attrs, fx, fy, fontWidth) {
        const gw = textWidth(label, fontWidth);
        const d = dirOutPathD(gw);
        const xLabel = -Math.round((gw + 20) / 2);
        const groupAttrs = { transform: "translate(" + fx + "," + fy + ")" };
        applyNavAttrs(groupAttrs, attrs);
        return [
          "g",
          groupAttrs,
          ["title", ...tspan.parse(label)],
          ["path", { class: "port", d }],
          ["text", { x: xLabel, y: 4, class: "bodylabel" }, ...tspan.parse(label)]
        ];
      }
      function leafNodeOf(branch) {
        return Array.isArray(branch) ? branch[0] : branch;
      }
      function drawLeaf(branch, fontWidth) {
        const node = leafNodeOf(branch);
        const displayName = leafDisplay(branch);
        const fx = node.fx;
        const fy = node.fy;
        const attrs = getAttrs(branch) || {};
        if (Array.isArray(branch) && isPinOp(node.name)) {
          return drawPortBox(branch, fx, fy, fontWidth);
        }
        if (attrs.dir === "in") {
          return drawDirInBox(displayName, attrs, fx, fy, fontWidth);
        }
        const boxW = getLabelWidth(displayName, fontWidth) + 2 * LEAF_PAD_X;
        const groupAttrs = { transform: "translate(" + fx + "," + fy + ")" };
        if (attrs.nodeId || attrs.siteKey) {
          groupAttrs.class = "rtl-node";
          if (attrs.nodeId) groupAttrs["data-node-id"] = attrs.nodeId;
          if (attrs.siteKey) groupAttrs["data-site-key"] = attrs.siteKey;
          if (attrs.module) groupAttrs["data-module"] = attrs.module;
          if (attrs.name) groupAttrs["data-name"] = attrs.name;
          if (attrs.loc) groupAttrs["data-loc"] = attrs.loc;
        }
        return [
          "g",
          groupAttrs,
          ["title", ...tspan.parse(node.name)],
          ["rect", {
            class: "siglabel",
            x: -boxW,
            y: -LEAF_HALF_H,
            width: boxW,
            height: 2 * LEAF_HALF_H
          }],
          ["text", { x: -LEAF_PAD_X, y: 4, class: "pinname" }, ...tspan.parse(displayName)]
        ];
      }
      function outBoxWidth(displayName, fontWidth) {
        return Math.max(getLabelWidth(displayName, fontWidth) + 2 * LEAF_PAD_X, OUT_MIN_BOX_W);
      }
      function drawOutLabel(tree, fx, fy, fontWidth) {
        const displayName = outDisplay(tree);
        const tooltip = outTooltip(tree);
        const boxW = outBoxWidth(displayName, fontWidth);
        const attrs = getAttrs(tree) || {};
        const groupAttrs = { transform: "translate(" + fx + "," + fy + ")" };
        if (attrs.nodeId) {
          groupAttrs.class = "rtl-node";
          groupAttrs["data-node-id"] = attrs.nodeId;
          if (attrs.module) groupAttrs["data-module"] = attrs.module;
          if (attrs.name) groupAttrs["data-name"] = attrs.name;
          if (attrs.loc) groupAttrs["data-loc"] = attrs.loc;
        }
        const group = [
          "g",
          groupAttrs,
          ["title", ...tspan.parse(tooltip)],
          ["rect", {
            class: "siglabel",
            x: 0,
            y: -LEAF_HALF_H,
            width: boxW,
            height: 2 * LEAF_HALF_H
          }]
        ];
        if (displayName) {
          group.push(["text", { x: LEAF_PAD_X, y: 4, class: "wirename" }, ...tspan.parse(displayName)]);
        }
        return group;
      }
      function inlineBoxWidth(visibleName, fontWidth) {
        if (!visibleName) return 16;
        return Math.max(getLabelWidth(visibleName, fontWidth) + 2 * LEAF_PAD_X, INLINE_MIN_BOX_W);
      }
      function drawInlineBox(tree, fx, fy, fontWidth) {
        const attrs = getAttrs(tree) || {};
        const start = firstChildIdx(tree);
        const nameBranch = tree[start];
        const node = leafNodeOf(nameBranch);
        const visibleName = node && node.name || "";
        const displayName = inlineDisplay(tree);
        const tooltip = attrs.label || visibleName;
        const boxW = inlineBoxWidth(displayName, fontWidth);
        const groupAttrs = { transform: "translate(" + fx + "," + fy + ")" };
        if (attrs.nodeId) {
          groupAttrs.class = "rtl-node";
          groupAttrs["data-node-id"] = attrs.nodeId;
          if (attrs.module) groupAttrs["data-module"] = attrs.module;
          if (attrs.name) groupAttrs["data-name"] = attrs.name;
          if (attrs.loc) groupAttrs["data-loc"] = attrs.loc;
        }
        const group = [
          "g",
          groupAttrs,
          ["title", ...tspan.parse(tooltip)],
          ["rect", {
            class: "siglabel",
            x: -boxW,
            y: -LEAF_HALF_H,
            width: boxW,
            height: 2 * LEAF_HALF_H
          }]
        ];
        if (displayName) {
          group.push(["text", { x: -boxW / 2, y: 4, class: "bodylabel" }, ...tspan.parse(displayName)]);
        }
        return group;
      }
      function shiftFxSubtree(branch, delta) {
        if (!branch) return;
        if (Array.isArray(branch)) {
          const node = leafNodeOf(branch);
          if (node && typeof node.fx === "number") node.fx -= delta;
          const start = firstChildIdx(branch);
          const ilen = branch.length;
          for (let i = start; i < ilen; i++) {
            shiftFxSubtree(branch[i], delta);
          }
        } else if (typeof branch === "object") {
          if (typeof branch.fx === "number") branch.fx -= delta;
        }
      }
      function childSpec(branch, fontWidth) {
        if (Array.isArray(branch) && branch[0]) {
          const op = branch[0].name;
          const start = firstChildIdx(branch);
          const isEq = op === "=" && branch.length > start + 1;
          const isPin = isPinOp(op) && branch.length > start;
          if (isEq || isPin) {
            const exprBranch = isPin ? branch[start] : branch[start + 1];
            const [, exprFy] = childSpec(exprBranch, fontWidth);
            const node2 = leafNodeOf(branch);
            return [node2.fx, exprFy, getWidth(branch)];
          }
        }
        const node = leafNodeOf(branch);
        let fx = node.fx;
        let fy = node.fy;
        if (node && typeof node.name === "string") {
          const body = drawBody(node.name, 0, 0, fontWidth);
          const o = body[1].o;
          fy += o || 0;
        }
        return [fx, fy, getWidth(branch)];
      }
      function drawAssign(tree, xmax, start, isRoot, fontWidth) {
        const op = tree[0].name;
        const pinOp = isPinOp(op);
        const attrs = getAttrs(tree) || {};
        const dir = attrs.dir;
        const gateFx = tree[0].fx;
        const nameBranch = pinOp ? null : tree[start];
        const exprBranch = pinOp ? tree[start] : tree[start + 1];
        let [exprFx, exprFy] = childSpec(exprBranch, fontWidth);
        const exprW = getWidth(exprBranch);
        let boxW = 0;
        let shapeKind;
        if (pinOp) {
          const labels = pinLabels(op, attrs);
          boxW = portBoxWidth(
            textWidth(labels.instance, fontWidth),
            textWidth(labels.pin, fontWidth)
          );
          shapeKind = "port";
        } else if (dir === "out") {
          boxW = dirOutBoxWidth(textWidth(outDisplay(tree), fontWidth));
          shapeKind = "dir-out";
        } else if (dir === "in") {
          boxW = dirInBoxWidth(textWidth(outDisplay(tree), fontWidth));
          shapeKind = "dir-in";
        } else if (!isRoot) {
          const node = leafNodeOf(nameBranch);
          const visibleName = node && node.name || "";
          boxW = inlineBoxWidth(visibleName, fontWidth);
          shapeKind = "inline";
        } else {
          shapeKind = "out";
        }
        if (!isRoot && boxW > 0) {
          const gap = gateFx - exprFx;
          const need = boxW + MIN_PASSTHRU_PX - gap;
          if (need > 0) {
            shiftFxSubtree(exprBranch, need);
            [exprFx, exprFy] = childSpec(exprBranch, fontWidth);
          }
        }
        const ret = ["g"];
        const shapeFy = exprFy;
        const passthruEndX = isRoot ? gateFx : gateFx - boxW;
        let passthruD;
        if (exprFy === shapeFy) {
          passthruD = "M" + exprFx + "," + exprFy + " H" + passthruEndX;
        } else {
          const half = (passthruEndX - exprFx) / 2;
          passthruD = "M" + exprFx + "," + exprFy + " h" + half + " v" + (shapeFy - exprFy) + " h" + half;
        }
        const passthru = ["path", {
          d: passthruD,
          class: ["wire", widther(exprW)]
        }];
        if (exprW > 1) passthru.push(["title", exprW + " bits"]);
        ret.push(passthru);
        ret.push(drawBoxes(exprBranch, xmax, false, fontWidth));
        const shapeAnchorX = isRoot ? gateFx + boxW : gateFx;
        ret.push(drawShape(shapeKind, tree, attrs, shapeAnchorX, gateFx, shapeFy, fontWidth));
        return ret;
      }
      function drawShape(shapeKind, tree, attrs, shapeAnchorX, gateFx, shapeFy, fontWidth) {
        switch (shapeKind) {
          case "port":
            return drawPortBox(tree, shapeAnchorX, shapeFy, fontWidth);
          case "dir-out":
            return drawDirOutBox(outDisplay(tree), attrs, shapeAnchorX, shapeFy, fontWidth);
          case "dir-in":
            return drawDirInBox(outDisplay(tree), attrs, shapeAnchorX, shapeFy, fontWidth);
          case "inline":
            return drawInlineBox(tree, gateFx, shapeFy, fontWidth);
          default:
            return drawOutLabel(tree, gateFx, shapeFy, fontWidth);
        }
      }
      var drawBoxesCallCount = 0;
      var MAX_DRAWBOXES_CALLS = 1e6;
      function resetDrawBoxesCallCount() {
        drawBoxesCallCount = 0;
      }
      function drawBoxes(tree, xmax, isRoot, fontWidth) {
        drawBoxesCallCount++;
        if (drawBoxesCallCount > MAX_DRAWBOXES_CALLS) {
          if (typeof console !== "undefined" && console.error) {
            console.error("drawBoxes exceeded " + MAX_DRAWBOXES_CALLS + " calls - possible infinite loop");
          }
          throw new Error("drawBoxes exceeded " + MAX_DRAWBOXES_CALLS + " calls");
        }
        if (Array.isArray(tree)) {
          const start = firstChildIdx(tree);
          const ilen = tree.length;
          if (ilen === start) {
            return ["g", drawLeaf(tree, fontWidth)];
          }
          if (tree[0].name === "=" && ilen > start + 1) {
            return drawAssign(tree, xmax, start, isRoot, fontWidth);
          }
          if (isPinOp(tree[0].name) && ilen > start) {
            return drawAssign(tree, xmax, start, isRoot, fontWidth);
          }
          const spec = [];
          spec.push(tree[0].name);
          spec.push([tree[0].fx, tree[0].fy, getWidth(tree)]);
          for (let i = start; i < ilen; i++) {
            spec.push(childSpec(tree[i], fontWidth));
          }
          const ret = ["g", drawGate(spec, getAttrs(tree))];
          for (let i = start; i < ilen; i++) {
            ret.push(drawBoxes(tree[i], xmax, false, fontWidth));
          }
          return ret;
        }
        return ["g", drawLeaf(tree, fontWidth)];
      }
      module2.exports = drawBoxes;
      module2.exports.resetCallCount = resetDrawBoxesCallCount;
      module2.exports.portBoxWidth = portBoxWidth;
      module2.exports.dirInBoxWidth = dirInBoxWidth;
      module2.exports.dirOutBoxWidth = dirOutBoxWidth;
      module2.exports.textWidth = textWidth;
    }
  });

  // node_modules/logidrom/lib/insert-svg-template-assign.js
  var require_insert_svg_template_assign = __commonJS({
    "node_modules/logidrom/lib/insert-svg-template-assign.js"(exports2, module2) {
      "use strict";
      function insertSVGTemplateAssign() {
        return ["style", ".pinname {font-size:12px; font-style:normal; font-variant:normal; font-weight:500; font-stretch:normal; text-align:center; text-anchor:end; font-family:monospace} .wirename {font-size:12px; font-style:normal; font-variant:normal; font-weight:500; font-stretch:normal; text-align:center; text-anchor:start; font-family:monospace} .wirename:hover {fill:blue} .gate {color:#000; fill:#aaa; fill-opacity: 1;stroke:#000; stroke-width:1; stroke-opacity:1} .dff {color:#000; fill:#777; fill-opacity: 1; stroke:#000; stroke-width:1; stroke-opacity:1} .dff:hover {fill:#ff7 !important; } .gate:hover {fill:red !important; } .port {color:#000; fill:#cce; fill-opacity:1; stroke:#000; stroke-width:1; stroke-opacity:1} .port:hover {fill:#aaf !important; } .bind {color:#000; fill:#ecc; fill-opacity:1; stroke:#000; stroke-width:1; stroke-opacity:1} .bind:hover {fill:#faa !important; } .siglabel {fill:#eee; fill-opacity:1; stroke:#ccc; stroke-width:1} .slicelabel {font-size:12px; font-family:monospace; text-anchor:middle; font-weight:500; paint-order:stroke; stroke:#fff; stroke-width:3; fill:#000} .bodylabel {font-size:12px; font-family:monospace; text-anchor:middle; font-weight:500; fill:#000} .wire {fill:none; stroke:#000; stroke-width:1; stroke-opacity:1} .wire.vector {stroke-width:3} .wire.zeroer {stroke-width:0.5; stroke-dasharray:2,2} .grid {fill:#fff; fill-opacity:1; stroke:none}"];
      }
      module2.exports = insertSVGTemplateAssign;
    }
  });

  // node_modules/logidrom/lib/render-assign.js
  var require_render_assign = __commonJS({
    "node_modules/logidrom/lib/render-assign.js"(exports2, module2) {
      "use strict";
      var render = require_render();
      var drawBoxes = require_draw_boxes();
      var drawBody = require_draw_body();
      var insertSVGTemplateAssign = require_insert_svg_template_assign();
      var { getLabelWidth } = require_font_metrics();
      var { firstChildIdx, getAttrs, leafDisplay, outDisplay, inlineDisplay, isPinOp, pinLabels } = require_tree_utils();
      var { portBoxWidth, dirInBoxWidth, dirOutBoxWidth, textWidth } = drawBoxes;
      var grid = 32;
      var ceilGrid = (n) => grid * Math.ceil(n / grid);
      var BOX_PAD_X = 4;
      var MIN_BOX_W = 32;
      var MIN_PASSTHRU_PX = 48;
      var boxW = (visible, fontWidth) => Math.max(getLabelWidth(visible, fontWidth) + 2 * BOX_PAD_X, MIN_BOX_W);
      var outBoxW = (tree, fontWidth) => boxW(outDisplay(tree), fontWidth);
      var leafBoxW = (visible, fontWidth) => getLabelWidth(visible, fontWidth) + 2 * BOX_PAD_X;
      var inlineBoxW = (visible, fontWidth) => visible ? boxW(visible, fontWidth) : 16;
      var leafNodeOf = (branch) => Array.isArray(branch) ? branch[0] : branch;
      var gateSize = (type, fontWidth, attrs) => {
        const body = drawBody(type, 0, 0, fontWidth, attrs);
        return {
          w: body[1].w || 0,
          h: body[1].h || 0
        };
      };
      var eqBoxW = (node, fontWidth, midTree) => {
        const attrs = getAttrs(node) || {};
        if (attrs.dir === "out") return dirOutBoxWidth(textWidth(outDisplay(node), fontWidth));
        if (attrs.dir === "in") return dirInBoxWidth(textWidth(outDisplay(node), fontWidth));
        return midTree ? inlineBoxW(inlineDisplay(node), fontWidth) : outBoxW(node, fontWidth);
      };
      var portBoxW = (node, fontWidth) => {
        const attrs = getAttrs(node) || {};
        const labels = pinLabels(node[0].name, attrs);
        return portBoxWidth(
          textWidth(labels.instance, fontWidth),
          textWidth(labels.pin, fontWidth)
        );
      };
      var leafConeW = (node, fontWidth) => {
        const op = node[0] && node[0].name;
        if (isPinOp(op)) return portBoxW(node, fontWidth);
        const attrs = getAttrs(node);
        if (attrs && attrs.dir === "in") {
          return dirInBoxWidth(textWidth(leafDisplay(node), fontWidth));
        }
        return leafBoxW(leafDisplay(node), fontWidth);
      };
      var measureExtents = (node, acc, isRoot, fontWidth) => {
        if (!Array.isArray(node)) {
          const fx = node && typeof node.fx === "number" ? node.fx : 0;
          acc.left = Math.max(acc.left, leafBoxW(leafDisplay(node), fontWidth) - fx);
          return;
        }
        const start = firstChildIdx(node);
        const ilen = node.length;
        if (ilen === start) {
          const fx = node[0].fx || 0;
          acc.left = Math.max(acc.left, leafConeW(node, fontWidth) - fx);
          return;
        }
        if (node[0].name === "=" && ilen > start + 1) {
          if (isRoot) {
            acc.root = Math.max(acc.root, eqBoxW(node, fontWidth, false));
          } else {
            const fx = node[0].fx || 0;
            acc.left = Math.max(acc.left, eqBoxW(node, fontWidth, true) - fx);
          }
          measureExtents(node[start + 1], acc, false, fontWidth);
          return;
        }
        if (isPinOp(node[0].name) && ilen > start) {
          const w = portBoxW(node, fontWidth);
          if (isRoot) {
            acc.root = Math.max(acc.root, w);
          } else {
            const fx = node[0].fx || 0;
            acc.left = Math.max(acc.left, w - fx);
          }
          measureExtents(node[start], acc, false, fontWidth);
          return;
        }
        for (let i = start; i < ilen; i++) {
          measureExtents(node[i], acc, false, fontWidth);
        }
      };
      var getNumChannels = (node) => {
        let downward = 0;
        let upward = 0;
        const y = node[0].y;
        const start = firstChildIdx(node);
        const ilen = node.length;
        for (let i = start; i < ilen; i++) {
          const child = node[i];
          if (!Array.isArray(child)) continue;
          const inputYdx = y + (i - start - (ilen - start - 1) / 2) * 2;
          if (child[0].y > inputYdx) {
            downward++;
          } else if (child[0].y < inputYdx) {
            upward++;
          }
        }
        const nChannels = Math.max(downward, upward);
        return nChannels;
      };
      var collectSlacks = (node, slacks, fontWidth) => {
        if (!Array.isArray(node)) return;
        const start = firstChildIdx(node);
        const ilen = node.length;
        const nChildren = ilen - start;
        if (nChildren > 0) {
          const name = node[0].name;
          if (name === "=") {
            if (nChildren > 1) collectSlacks(node[start + 1], slacks, fontWidth);
            return;
          }
          if (isPinOp(name)) {
            if (nChildren > 0) collectSlacks(node[start], slacks, fontWidth);
            return;
          }
          const { w } = gateSize(name, fontWidth, getAttrs(node));
          const routingSpace = drawBody.isShape(name) ? 0 : getNumChannels(node) * 8;
          const extra = w + routingSpace - (grid + 1 >> 1);
          if (extra > 0) {
            const col = node[0].x;
            slacks[col] = Math.max(slacks[col] || 0, extra);
          }
        }
        for (let i = start; i < ilen; i++) {
          collectSlacks(node[i], slacks, fontWidth);
        }
      };
      var shiftFxSubtree = (branch, delta) => {
        if (!branch) return;
        if (Array.isArray(branch)) {
          const node = leafNodeOf(branch);
          if (node && typeof node.fx === "number") node.fx -= delta;
          const start = firstChildIdx(branch);
          const ilen = branch.length;
          for (let i = start; i < ilen; i++) {
            shiftFxSubtree(branch[i], delta);
          }
        } else if (typeof branch === "object") {
          if (typeof branch.fx === "number") branch.fx -= delta;
        }
      };
      var shiftInlineExprs = (node, isRoot, fontWidth) => {
        if (!Array.isArray(node)) return;
        const start = firstChildIdx(node);
        const ilen = node.length;
        const op = node[0].name;
        if (op === "=" && ilen > start + 1) {
          if (!isRoot) {
            const exprBranch = node[start + 1];
            const exprNode = leafNodeOf(exprBranch);
            const gap = node[0].fx - exprNode.fx;
            const need = eqBoxW(node, fontWidth, true) + MIN_PASSTHRU_PX - gap;
            if (need > 0) {
              shiftFxSubtree(exprBranch, need);
            }
          }
          shiftInlineExprs(node[start + 1], false, fontWidth);
          return;
        }
        if (isPinOp(op) && ilen > start) {
          if (!isRoot) {
            const exprBranch = node[start];
            const exprNode = leafNodeOf(exprBranch);
            const gap = node[0].fx - exprNode.fx;
            const need = portBoxW(node, fontWidth) + MIN_PASSTHRU_PX - gap;
            if (need > 0) {
              shiftFxSubtree(exprBranch, need);
            }
          }
          shiftInlineExprs(node[start], false, fontWidth);
          return;
        }
        for (let i = start; i < ilen; i++) {
          shiftInlineExprs(node[i], false, fontWidth);
        }
      };
      var computeTrailing = (slacks, xmax) => {
        const trailing = Array.from({ length: xmax + 1 }, () => 0);
        for (let x = xmax - 1; x >= 0; x--) {
          trailing[x] = trailing[x + 1] + (slacks[x] || 0);
        }
        return trailing;
      };
      var pixelFx = (x, xmax, trailing) => 32 * (xmax - x) + (trailing[x] || 0);
      var setNodeFxFy = (layoutNode, xmax, trailing) => {
        layoutNode.fx = pixelFx(layoutNode.x, xmax, trailing);
        layoutNode.fy = 8 * layoutNode.y;
      };
      var assignFx = (node, xmax, trailing) => {
        if (!Array.isArray(node)) {
          setNodeFxFy(node, xmax, trailing);
          return;
        }
        const start = firstChildIdx(node);
        const ilen = node.length;
        setNodeFxFy(node[0], xmax, trailing);
        if (node[0].name === "=" && ilen > start + 1) {
          const nameBranch = node[start];
          const nameNode = Array.isArray(nameBranch) ? nameBranch[0] : nameBranch;
          setNodeFxFy(nameNode, xmax, trailing);
          assignFx(node[start + 1], xmax, trailing);
          return;
        }
        for (let i = start; i < ilen; i++) {
          assignFx(node[i], xmax, trailing);
        }
      };
      function renderAssign(index, source2, notFirstAssign) {
        if (drawBoxes.resetCallCount) drawBoxes.resetCallCount();
        let state = { x: 0, y: 2, xmax: 0 };
        const tree = source2.assign;
        const config = source2.config || {};
        const fontWidth = config.fontWidth || 7.23;
        const treeSpacing = config.treeSpacing || 16;
        const ilen = tree.length;
        const treeSpacingY = Math.round(treeSpacing / 8);
        for (let i = 0; i < ilen; i++) {
          state = render(tree[i], state);
          state.x++;
          if (i < ilen - 1) {
            state.y += treeSpacingY;
          }
        }
        const xmax = state.xmax;
        const trailings = Array.from({ length: ilen }, () => 0);
        let totalSlack = 0;
        for (let i = 0; i < ilen; i++) {
          const slacks = [];
          collectSlacks(tree[i], slacks, fontWidth);
          const trailing = computeTrailing(slacks, xmax);
          trailings[i] = trailing;
          const coneSlack = trailing[0] || 0;
          if (coneSlack > totalSlack) totalSlack = coneSlack;
        }
        const acc = { left: 0, root: 0 };
        for (let i = 0; i < ilen; i++) {
          assignFx(tree[i], xmax, trailings[i]);
          shiftInlineExprs(tree[i], true, fontWidth);
          measureExtents(tree[i], acc, true, fontWidth);
        }
        const leftPad = ceilGrid(Math.max(0, acc.left));
        const rightPad = ceilGrid(Math.max(0, acc.root - (grid + 1)));
        const svg = ["g"];
        for (let i = 0; i < ilen; i++) {
          svg.push(drawBoxes(tree[i], xmax, true, fontWidth));
        }
        const width = leftPad + 32 * (xmax + 1) + 1 + rightPad + totalSlack;
        const height = 8 * (state.y + 1) - 7;
        return [
          "svg",
          {
            id: "svgcontent_" + index,
            viewBox: "0 0 " + width + " " + height,
            width,
            height
          },
          // Embed style unless caller says an earlier assign diagram on the same page
          // already has it. Standalone renders (ObservableHQ, any index) need it,
          // otherwise wires get default fill:black.
          ...notFirstAssign ? [] : [insertSVGTemplateAssign()],
          ["g", { transform: "translate(" + (leftPad + 0.5) + ", 0.5)" }, svg]
        ];
      }
      module2.exports = renderAssign;
    }
  });

  // node_modules/bit-field/lib/render.js
  var require_render2 = __commonJS({
    "node_modules/bit-field/lib/render.js"(exports2, module2) {
      "use strict";
      var tspan = require_lib();
      var round = Math.round;
      var getSVG = (w, h) => ["svg", {
        xmlns: "http://www.w3.org/2000/svg",
        // TODO link ns?
        width: w,
        height: h,
        viewBox: [0, 0, w, h].join(" ")
      }];
      var tt = (x, y, obj) => Object.assign(
        { transform: "translate(" + x + (y ? "," + y : "") + ")" },
        typeof obj === "object" ? obj : {}
      );
      var colors = {
        // TODO compare with WaveDrom
        2: "#ff0000",
        // 'hsl(0,100%,50%)'
        3: "#aaff00",
        // 'hsl(80,100%,50%)'
        4: "#00ffd5",
        // 'hsl(170,100%,50%)'
        5: "#ffbf00",
        // 'hsl(45,100%,50%)'
        6: "#00ff19",
        // 'hsl(126,100%,50%)'
        7: "#006aff"
        // 'hsl(215,100%,50%)'
      };
      var typeStyle = (t) => colors[t] !== void 0 ? ";fill:" + colors[t] : "";
      var norm = (obj, other) => Object.assign(
        Object.keys(obj).reduce((prev, key) => {
          const val = Number(obj[key]);
          const valInt = isNaN(val) ? 0 : Math.round(val);
          if (valInt !== 0) {
            prev[key] = valInt;
          }
          return prev;
        }, {}),
        other
      );
      var trimText = (text2, availableSpace, charWidth) => {
        if (!(typeof text2 === "string" || text2 instanceof String))
          return text2;
        const textWidth = text2.length * charWidth;
        if (textWidth <= availableSpace)
          return text2;
        var end = text2.length - (textWidth - availableSpace) / charWidth - 3;
        if (end > 0)
          return text2.substring(0, round(end)) + "...";
        return text2.substring(0, 1) + "...";
      };
      var text = (body, x, y, rotate) => {
        const props = { y: 6 };
        if (rotate !== void 0) {
          props.transform = "rotate(" + rotate + ")";
        }
        return ["g", tt(round(x), round(y)), ["text", props].concat(tspan.parse(body))];
      };
      var hline = (len, x, y) => ["line", norm({ x1: x, x2: x + len, y1: y, y2: y })];
      var vline = (len, x, y) => ["line", norm({ x1: x, x2: x, y1: y, y2: y + len })];
      var getLabel = (val, x, y, step, len, rotate) => {
        if (typeof val !== "number") {
          return text(val, x, y, rotate);
        }
        const res = ["g", {}];
        for (let i = 0; i < len; i++) {
          res.push(text(
            val >> i & 1,
            x + step * (len / 2 - i - 0.5),
            y
          ));
        }
        return res;
      };
      var getAttr = (e, opt, step, lsbm, msbm) => {
        const x = opt.vflip ? step * ((msbm + lsbm) / 2) : step * (opt.mod - (msbm + lsbm) / 2 - 1);
        if (!Array.isArray(e.attr)) {
          return getLabel(e.attr, x, 0, step, e.bits);
        }
        return e.attr.reduce(
          (prev, a, i) => a === void 0 || a === null ? prev : prev.concat([getLabel(a, x, opt.fontsize * i, step, e.bits)]),
          ["g", {}]
        );
      };
      var labelArr = (desc, opt) => {
        const { margin, hspace, vspace, mod, index, fontsize, vflip, trim, compact, offset } = opt;
        const width = hspace - margin.left - margin.right - 1;
        const height = vspace - margin.top - margin.bottom;
        const step = width / mod;
        const blanks = ["g"];
        const bits = ["g", tt(round(step / 2), -round(0.5 * fontsize + 4))];
        const names = ["g", tt(round(step / 2), round(0.5 * height + 0.4 * fontsize - 6))];
        const attrs = ["g", tt(round(step / 2), round(height + 0.7 * fontsize - 2))];
        desc.map((e) => {
          let lsbm = 0;
          let msbm = mod - 1;
          let lsb = index * mod;
          let msb = (index + 1) * mod - 1;
          if (e.lsb / mod >> 0 === index) {
            lsbm = e.lsbm;
            lsb = e.lsb;
            if (e.msb / mod >> 0 === index) {
              msb = e.msb;
              msbm = e.msbm;
            }
          } else {
            if (e.msb / mod >> 0 === index) {
              msb = e.msb;
              msbm = e.msbm;
            } else if (!(lsb > e.lsb && msb < e.msb)) {
              return;
            }
          }
          if (!compact) {
            bits.push(text(lsb + offset, step * (vflip ? lsbm : mod - lsbm - 1)));
            if (lsbm !== msbm) {
              bits.push(text(msb + offset, step * (vflip ? msbm : mod - msbm - 1)));
            }
          }
          if (e.name !== void 0) {
            names.push(getLabel(
              trim ? trimText(e.name, step * e.bits, trim) : e.name,
              step * (vflip ? (msbm + lsbm) / 2 : mod - (msbm + lsbm) / 2 - 1),
              0,
              step,
              e.bits,
              e.rotate
            ));
          }
          if (e.name === void 0 || e.type !== void 0) {
            if (!(opt.compact && e.type === void 0)) {
              blanks.push(["rect", Object.assign(
                {},
                norm({
                  x: step * (vflip ? lsbm : mod - msbm - 1),
                  width: step * (msbm - lsbm + 1),
                  height
                }, {
                  field: e.name,
                  style: "fill-opacity:0.1" + typeStyle(e.type)
                }),
                e.rect !== void 0 ? e.rect : {}
              )]);
            }
          }
          if (e.attr !== void 0) {
            attrs.push(getAttr(e, opt, step, lsbm, msbm));
          }
        });
        return ["g", blanks, bits, names, attrs];
      };
      var getLabelMask = (desc, mod) => {
        const mask = [];
        let idx = 0;
        desc.map((e) => {
          mask[idx % mod] = true;
          idx += e.bits;
          mask[(idx - 1) % mod] = true;
        });
        return mask;
      };
      var getLegendItems = (opt) => {
        const { hspace, margin, fontsize, legend } = opt;
        const width = hspace - margin.left - margin.right - 1;
        const items = ["g", tt(margin.left, -10)];
        const legendSquarePadding = 36;
        const legendNamePadding = 24;
        let x = width / 2 - Object.keys(legend).length / 2 * (legendSquarePadding + legendNamePadding);
        for (const key in legend) {
          const value = legend[key];
          items.push(["rect", norm({
            x,
            width: 12,
            height: 12
          }, {
            style: "fill-opacity:0.15; stroke: #000; stroke-width: 1.2;" + typeStyle(value)
          })]);
          x += legendSquarePadding;
          items.push(text(
            key,
            x,
            0.1 * fontsize + 4
          ));
          x += legendNamePadding;
        }
        return items;
      };
      var compactLabels = (desc, opt) => {
        const { hspace, margin, mod, fontsize, vflip, legend, offset } = opt;
        const width = hspace - margin.left - margin.right - 1;
        const step = width / mod;
        const labels = ["g", tt(margin.left, legend ? 0 : -3)];
        const mask = getLabelMask(desc, mod);
        for (let i = 0; i < mod; i++) {
          const idx = vflip ? i : mod - i - 1;
          if (mask[idx]) {
            labels.push(text(
              idx + offset,
              step * (i + 0.5),
              0.5 * fontsize + 4
            ));
          }
        }
        return labels;
      };
      var skipField = (desc, opt, globalIndex) => {
        if (!opt.compact) {
          return false;
        }
        const emptyField = (e) => e.name === void 0 && e.type === void 0;
        if (desc.findIndex((e) => emptyField(e) && globalIndex > e.lsb && globalIndex <= e.msb + 1) !== -1) {
          return true;
        }
        return false;
      };
      var cage = (desc, opt) => {
        const { hspace, vspace, mod, margin, index, vflip } = opt;
        const width = hspace - margin.left - margin.right - 1;
        const height = vspace - margin.top - margin.bottom;
        const res = [
          "g",
          {
            stroke: "black",
            "stroke-width": 1,
            "stroke-linecap": "round"
          }
        ];
        if (opt.sparse) {
          const skipEdge = opt.uneven && opt.bits % 2 === 1 && index === opt.lanes - 1;
          if (skipEdge) {
            if (vflip) {
              res.push(
                hline(width - width / mod, 0, 0),
                hline(width - width / mod, 0, height)
              );
            } else {
              res.push(
                hline(width - width / mod, width / mod, 0),
                hline(width - width / mod, width / mod, height)
              );
            }
          } else if (!opt.compact) {
            res.push(
              hline(width, 0, 0),
              hline(width, 0, height),
              vline(height, vflip ? width : 0, 0)
            );
          }
        } else {
          res.push(
            hline(width, 0, 0),
            vline(height, vflip ? width : 0, 0),
            hline(width, 0, height)
          );
        }
        let i = index * mod;
        const delta = vflip ? 1 : -1;
        let j = vflip ? 0 : mod;
        if (opt.sparse) {
          for (let k = 0; k <= mod; k++) {
            const xj = j * (width / mod);
            if (!skipField(desc, opt, i) && k !== 0 || !skipField(desc, opt, i + 1) && k !== mod) {
              if (k === 0 || k === mod || desc.some((e) => e.msb + 1 === i)) {
                res.push(vline(height, xj, 0));
              } else {
                res.push(vline(height >>> 3, xj, 0));
                res.push(vline(-(height >>> 3), xj, height));
              }
            }
            if (opt.compact && k !== 0 && !skipField(desc, opt, i)) {
              res.push(hline(width / mod, xj, 0));
              res.push(hline(width / mod, xj, height));
            }
            i++;
            j += delta;
          }
        } else {
          for (let k = 0; k < mod; k++) {
            const xj = j * (width / mod);
            if (k === 0 || desc.some((e) => e.lsb === i)) {
              res.push(vline(height, xj, 0));
            } else {
              res.push(
                vline(height >>> 3, xj, 0),
                vline(-(height >>> 3), xj, height)
              );
            }
            i++;
            j += delta;
          }
        }
        return res;
      };
      var lane = (desc, opt) => {
        const { index, vspace, hspace, margin, hflip, lanes, compact, label } = opt;
        const height = vspace - margin.top - margin.bottom;
        const width = hspace - margin.left - margin.right - 1;
        let tx = margin.left;
        const idx = hflip ? index : lanes - index - 1;
        let ty = round(idx * vspace + margin.top);
        if (compact) {
          ty = round(idx * height + margin.top);
        }
        const res = [
          "g",
          tt(tx, ty),
          cage(desc, opt),
          labelArr(desc, opt)
        ];
        if (label && label.left !== void 0) {
          const lab = label.left;
          let txt = index;
          if (typeof lab === "string") {
            txt = lab;
          } else if (typeof lab === "number") {
            txt += lab;
          } else if (typeof lab === "object") {
            txt = lab[index] || txt;
          }
          res.push([
            "g",
            { "text-anchor": "end" },
            text(txt, -4, round(height / 2))
          ]);
        }
        if (label && label.right !== void 0) {
          const lab = label.right;
          let txt = index;
          if (typeof lab === "string") {
            txt = lab;
          } else if (typeof lab === "number") {
            txt += lab;
          } else if (typeof lab === "object") {
            txt = lab[index] || txt;
          }
          res.push([
            "g",
            { "text-anchor": "start" },
            text(txt, width + 4, round(height / 2))
          ]);
        }
        return res;
      };
      var getMaxAttributes = (desc) => desc.reduce(
        (prev, field) => Math.max(
          prev,
          field.attr === void 0 ? 0 : Array.isArray(field.attr) ? field.attr.length : 1
        ),
        0
      );
      var getTotalBits = (desc) => desc.reduce((prev, field) => prev + (field.bits === void 0 ? 0 : field.bits), 0);
      var isIntGTorDefault = (opt) => (row) => {
        const [key, min, def] = row;
        const val = Math.round(opt[key]);
        opt[key] = typeof val === "number" && val >= min ? val : def;
      };
      var optDefaults = (opt) => {
        opt = typeof opt === "object" ? opt : {};
        [
          // key         min default
          // ['vspace', 20, 60],
          ["hspace", 40, 800],
          ["lanes", 1, 1],
          ["bits", 1, void 0],
          ["fontsize", 6, 14]
        ].map(isIntGTorDefault(opt));
        opt.fontfamily = opt.fontfamily || "sans-serif";
        opt.fontweight = opt.fontweight || "normal";
        opt.compact = opt.compact || false;
        opt.hflip = opt.hflip || false;
        opt.uneven = opt.uneven || false;
        opt.margin = opt.margin || {};
        opt.offset = opt.offset || 0;
        return opt;
      };
      var render = (desc, opt) => {
        opt = optDefaults(opt);
        const maxAttributes = getMaxAttributes(desc);
        opt.vspace = opt.vspace || (maxAttributes + 4) * opt.fontsize;
        if (opt.bits === void 0) {
          opt.bits = getTotalBits(desc);
        }
        const { hspace, vspace, lanes, margin, compact, fontsize, bits, label, legend } = opt;
        if (margin.right === void 0) {
          if (label && label.right !== void 0) {
            margin.right = round(0.1 * hspace);
          } else {
            margin.right = 4;
          }
        }
        if (margin.left === void 0) {
          if (label && label.left !== void 0) {
            margin.left = round(0.1 * hspace);
          } else {
            margin.left = 4;
          }
        }
        if (margin.top === void 0) {
          margin.top = 1.5 * fontsize;
          if (margin.bottom === void 0) {
            margin.bottom = fontsize * maxAttributes + 4;
          }
        } else {
          if (margin.bottom === void 0) {
            margin.bottom = 4;
          }
        }
        const width = hspace;
        let height = vspace * lanes;
        if (compact) {
          height -= (lanes - 1) * (margin.top + margin.bottom);
        }
        if (legend) {
          height += 12;
        }
        const res = [
          "g",
          tt(0.5, legend ? 12.5 : 0.5, {
            "text-anchor": "middle",
            "font-size": opt.fontsize,
            "font-family": opt.fontfamily,
            "font-weight": opt.fontweight
          })
        ];
        let lsb = 0;
        const mod = Math.ceil(bits * 1 / lanes);
        opt.mod = mod | 0;
        desc.map((e) => {
          e.lsb = lsb;
          e.lsbm = lsb % mod;
          lsb += e.bits;
          e.msb = lsb - 1;
          e.msbm = e.msb % mod;
        });
        for (let i = 0; i < lanes; i++) {
          opt.index = i;
          res.push(lane(desc, opt));
        }
        if (compact) {
          res.push(compactLabels(desc, opt));
        }
        if (legend) {
          res.push(getLegendItems(opt));
        }
        return getSVG(width, height).concat([res]);
      };
      module2.exports = render;
    }
  });

  // lib/render-reg.js
  var require_render_reg = __commonJS({
    "lib/render-reg.js"(exports2, module2) {
      "use strict";
      var render = require_render2();
      function renderReg(index, source2) {
        return render(source2.reg, source2.config);
      }
      module2.exports = renderReg;
    }
  });

  // lib/rec.js
  var require_rec = __commonJS({
    "lib/rec.js"(exports2, module2) {
      "use strict";
      function rec(tmp, state) {
        let deltaX = 10;
        let name;
        if (typeof tmp[0] === "string" || typeof tmp[0] === "number") {
          name = tmp[0];
          deltaX = 25;
        }
        state.x += deltaX;
        for (let i = 0; i < tmp.length; i++) {
          if (typeof tmp[i] === "object") {
            if (Array.isArray(tmp[i])) {
              const oldY = state.y;
              state = rec(tmp[i], state);
              state.groups.push({ x: state.xx, y: oldY, height: state.y - oldY, name: state.name });
            } else {
              state.lanes.push(tmp[i]);
              state.width.push(state.x);
              state.y += 1;
            }
          }
        }
        state.xx = state.x;
        state.x -= deltaX;
        state.name = name;
        return state;
      }
      module2.exports = rec;
    }
  });

  // lib/lane.js
  var require_lane = __commonJS({
    "lib/lane.js"(exports2, module2) {
      "use strict";
      var lane = {
        xs: 20,
        // tmpgraphlane0.width
        ys: 20,
        // tmpgraphlane0.height
        xg: 120,
        // tmpgraphlane0.x
        // yg     : 0,     // head gap
        yh0: 0,
        // head gap title
        yh1: 0,
        // head gap
        yf0: 0,
        // foot gap
        yf1: 0,
        // foot gap
        y0: 5,
        // tmpgraphlane0.y
        yo: 30,
        // tmpgraphlane1.y - y0;
        tgo: -10,
        // tmptextlane0.x - xg;
        ym: 15,
        // tmptextlane0.y - y0
        xlabel: 6,
        // tmptextlabel.x - xg;
        xmax: 1,
        scale: 1,
        head: {},
        foot: {}
      };
      module2.exports = lane;
    }
  });

  // lib/parse-config.js
  var require_parse_config = __commonJS({
    "lib/parse-config.js"(exports2, module2) {
      "use strict";
      function parseConfig(source2, lane) {
        function tonumber(x) {
          return x > 0 ? Math.round(x) : 1;
        }
        lane.hscale = 1;
        if (lane.hscale0) {
          lane.hscale = lane.hscale0;
        }
        if (source2 && source2.config && source2.config.hscale) {
          let hscale = Math.round(tonumber(source2.config.hscale));
          if (hscale > 0) {
            if (hscale > 100) {
              hscale = 100;
            }
            lane.hscale = hscale;
          }
        }
        lane.yh0 = 0;
        lane.yh1 = 0;
        lane.head = source2.head;
        lane.xmin_cfg = 0;
        lane.xmax_cfg = 1e12;
        if (source2 && source2.config && source2.config.hbounds && source2.config.hbounds.length == 2) {
          source2.config.hbounds[0] = Math.floor(source2.config.hbounds[0]);
          source2.config.hbounds[1] = Math.ceil(source2.config.hbounds[1]);
          if (source2.config.hbounds[0] < source2.config.hbounds[1]) {
            lane.xmin_cfg = 2 * Math.floor(source2.config.hbounds[0]);
            lane.xmax_cfg = 2 * Math.floor(source2.config.hbounds[1]);
          }
        }
        if (source2 && source2.head) {
          if (source2.head.tick || source2.head.tick === 0 || source2.head.tock || source2.head.tock === 0) {
            lane.yh0 = 20;
          }
          if (source2.head.tick || source2.head.tick === 0) {
            source2.head.tick = source2.head.tick + lane.xmin_cfg / 2;
          }
          if (source2.head.tock || source2.head.tock === 0) {
            source2.head.tock = source2.head.tock + lane.xmin_cfg / 2;
          }
          if (source2.head.text) {
            lane.yh1 = 46;
            lane.head.text = source2.head.text;
          }
        }
        lane.yf0 = 0;
        lane.yf1 = 0;
        lane.foot = source2.foot;
        if (source2 && source2.foot) {
          if (source2.foot.tick || source2.foot.tick === 0 || source2.foot.tock || source2.foot.tock === 0) {
            lane.yf0 = 20;
          }
          if (source2.foot.tick || source2.foot.tick === 0) {
            source2.foot.tick = source2.foot.tick + lane.xmin_cfg / 2;
          }
          if (source2.foot.tock || source2.foot.tock === 0) {
            source2.foot.tock = source2.foot.tock + lane.xmin_cfg / 2;
          }
          if (source2.foot.text) {
            lane.yf1 = 46;
            lane.foot.text = source2.foot.text;
          }
        }
      }
      module2.exports = parseConfig;
    }
  });

  // lib/gen-brick.js
  var require_gen_brick = __commonJS({
    "lib/gen-brick.js"(exports2, module2) {
      "use strict";
      var genBrick = (texts, extra, times) => {
        const R = [];
        if (!Array.isArray(texts)) {
          texts = [texts];
        }
        if (texts.length === 4) {
          for (let j = 0; j < times; j += 1) {
            R.push(texts[0]);
            for (let i = 0; i < extra; i += 1) {
              R.push(texts[1]);
            }
            R.push(texts[2]);
            for (let i = 0; i < extra; i += 1) {
              R.push(texts[3]);
            }
          }
          return R;
        }
        if (texts.length === 1) {
          texts.push(texts[0]);
        }
        R.push(texts[0]);
        for (let i = 0; i < times * (2 * (extra + 1)) - 1; i += 1) {
          R.push(texts[1]);
        }
        return R;
      };
      module2.exports = genBrick;
    }
  });

  // lib/gen-first-wave-brick.js
  var require_gen_first_wave_brick = __commonJS({
    "lib/gen-first-wave-brick.js"(exports2, module2) {
      "use strict";
      var genBrick = require_gen_brick();
      var lookUpTable = {
        p: ["pclk", "111", "nclk", "000"],
        n: ["nclk", "000", "pclk", "111"],
        P: ["Pclk", "111", "nclk", "000"],
        N: ["Nclk", "000", "pclk", "111"],
        l: "000",
        L: "000",
        0: "000",
        h: "111",
        H: "111",
        1: "111",
        "=": "vvv-2",
        2: "vvv-2",
        3: "vvv-3",
        4: "vvv-4",
        5: "vvv-5",
        6: "vvv-6",
        7: "vvv-7",
        8: "vvv-8",
        9: "vvv-9",
        d: "ddd",
        u: "uuu",
        z: "zzz",
        default: "xxx"
      };
      var genFirstWaveBrick = (text, extra, times) => genBrick(lookUpTable[text] || lookUpTable.default, extra, times);
      module2.exports = genFirstWaveBrick;
    }
  });

  // lib/gen-wave-brick.js
  var require_gen_wave_brick = __commonJS({
    "lib/gen-wave-brick.js"(exports2, module2) {
      "use strict";
      var genBrick = require_gen_brick();
      function genWaveBrick(text, extra, times) {
        const x1 = { p: "pclk", n: "nclk", P: "Pclk", N: "Nclk", h: "pclk", l: "nclk", H: "Pclk", L: "Nclk" };
        const x2 = {
          "0": "0",
          "1": "1",
          "x": "x",
          "d": "d",
          "u": "u",
          "z": "z",
          "=": "v",
          "2": "v",
          "3": "v",
          "4": "v",
          "5": "v",
          "6": "v",
          "7": "v",
          "8": "v",
          "9": "v"
        };
        const x3 = {
          "0": "",
          "1": "",
          "x": "",
          "d": "",
          "u": "",
          "z": "",
          "=": "-2",
          "2": "-2",
          "3": "-3",
          "4": "-4",
          "5": "-5",
          "6": "-6",
          "7": "-7",
          "8": "-8",
          "9": "-9"
        };
        const y1 = {
          "p": "0",
          "n": "1",
          "P": "0",
          "N": "1",
          "h": "1",
          "l": "0",
          "H": "1",
          "L": "0",
          "0": "0",
          "1": "1",
          "x": "x",
          "d": "d",
          "u": "u",
          "z": "z",
          "=": "v",
          "2": "v",
          "3": "v",
          "4": "v",
          "5": "v",
          "6": "v",
          "7": "v",
          "8": "v",
          "9": "v"
        };
        const y2 = {
          "p": "",
          "n": "",
          "P": "",
          "N": "",
          "h": "",
          "l": "",
          "H": "",
          "L": "",
          "0": "",
          "1": "",
          "x": "",
          "d": "",
          "u": "",
          "z": "",
          "=": "-2",
          "2": "-2",
          "3": "-3",
          "4": "-4",
          "5": "-5",
          "6": "-6",
          "7": "-7",
          "8": "-8",
          "9": "-9"
        };
        const x4 = {
          "p": "111",
          "n": "000",
          "P": "111",
          "N": "000",
          "h": "111",
          "l": "000",
          "H": "111",
          "L": "000",
          "0": "000",
          "1": "111",
          "x": "xxx",
          "d": "ddd",
          "u": "uuu",
          "z": "zzz",
          "=": "vvv-2",
          "2": "vvv-2",
          "3": "vvv-3",
          "4": "vvv-4",
          "5": "vvv-5",
          "6": "vvv-6",
          "7": "vvv-7",
          "8": "vvv-8",
          "9": "vvv-9"
        };
        const x5 = { p: "nclk", n: "pclk", P: "nclk", N: "pclk" };
        const x6 = { p: "000", n: "111", P: "000", N: "111" };
        const xclude = { hp: "111", Hp: "111", ln: "000", Ln: "000", nh: "111", Nh: "111", pl: "000", Pl: "000" };
        const atext = text.split("");
        const tmp0 = x4[atext[1]];
        let tmp1 = x1[atext[1]];
        if (tmp1 === void 0) {
          const tmp2 = x2[atext[1]];
          if (tmp2 === void 0) {
            return genBrick("xxx", extra, times);
          } else {
            const tmp3 = y1[atext[0]];
            if (tmp3 === void 0) {
              return genBrick("xxx", extra, times);
            }
            return genBrick([tmp3 + "m" + tmp2 + y2[atext[0]] + x3[atext[1]], tmp0], extra, times);
          }
        } else {
          const tmp4 = xclude[text];
          if (tmp4 !== void 0) {
            tmp1 = tmp4;
          }
          const tmp5 = x5[atext[1]];
          if (tmp5 === void 0) {
            return genBrick([tmp1, tmp0], extra, times);
          }
          return genBrick([tmp1, tmp0, tmp5, x6[atext[1]]], extra, times);
        }
      }
      module2.exports = genWaveBrick;
    }
  });

  // lib/find-lane-markers.js
  var require_find_lane_markers = __commonJS({
    "lib/find-lane-markers.js"(exports2, module2) {
      "use strict";
      function findLaneMarkers(lanetext) {
        let gcount = 0;
        let lcount = 0;
        const ret = [];
        lanetext.forEach(function(e) {
          if (e === "vvv-2" || e === "vvv-3" || e === "vvv-4" || e === "vvv-5" || e === "vvv-6" || e === "vvv-7" || e === "vvv-8" || e === "vvv-9") {
            lcount += 1;
          } else {
            if (lcount !== 0) {
              ret.push(gcount - (lcount + 1) / 2);
              lcount = 0;
            }
          }
          gcount += 1;
        });
        if (lcount !== 0) {
          ret.push(gcount - (lcount + 1) / 2);
        }
        return ret;
      }
      module2.exports = findLaneMarkers;
    }
  });

  // lib/parse-wave-lane.js
  var require_parse_wave_lane = __commonJS({
    "lib/parse-wave-lane.js"(exports2, module2) {
      "use strict";
      var genFirstWaveBrick = require_gen_first_wave_brick();
      var genWaveBrick = require_gen_wave_brick();
      var findLaneMarkers = require_find_lane_markers();
      function parseWaveLane(src, extra, lane) {
        const Stack = src.split("");
        let Next = Stack.shift();
        let Repeats = 1;
        while (Stack[0] === "." || Stack[0] === "|") {
          Stack.shift();
          Repeats += 1;
        }
        let R = [];
        R = R.concat(genFirstWaveBrick(Next, extra, Repeats));
        let Top;
        let subCycle = false;
        while (Stack.length) {
          Top = Next;
          Next = Stack.shift();
          if (Next === "<") {
            subCycle = true;
            Next = Stack.shift();
          }
          if (Next === ">") {
            subCycle = false;
            Next = Stack.shift();
          }
          Repeats = 1;
          while (Stack[0] === "." || Stack[0] === "|") {
            Stack.shift();
            Repeats += 1;
          }
          if (subCycle) {
            R = R.concat(genWaveBrick(Top + Next, 0, Repeats - lane.period));
          } else {
            R = R.concat(genWaveBrick(Top + Next, extra, Repeats));
          }
        }
        const unseen_bricks = [];
        for (let i = 0; i < lane.phase; i += 1) {
          unseen_bricks.push(R.shift());
        }
        let num_unseen_markers;
        if (unseen_bricks.length > 0) {
          num_unseen_markers = findLaneMarkers(unseen_bricks).length;
          if (findLaneMarkers([unseen_bricks[unseen_bricks.length - 1]]).length == 1 && findLaneMarkers([R[0]]).length == 1) {
            num_unseen_markers -= 1;
          }
        } else {
          num_unseen_markers = 0;
        }
        return [R, num_unseen_markers];
      }
      module2.exports = parseWaveLane;
    }
  });

  // lib/parse-wave-lanes.js
  var require_parse_wave_lanes = __commonJS({
    "lib/parse-wave-lanes.js"(exports2, module2) {
      "use strict";
      var parseWaveLane = require_parse_wave_lane();
      function data_extract(e, num_unseen_markers) {
        let ret_data = e.data;
        if (ret_data === void 0) {
          return null;
        }
        if (typeof ret_data === "string") {
          ret_data = ret_data.trim().split(/\s+/);
        }
        ret_data = ret_data.slice(num_unseen_markers);
        return ret_data;
      }
      function parseWaveLanes(sig, lane) {
        const content = [];
        const tmp0 = [];
        sig.map(function(sigx) {
          const current = [];
          content.push(current);
          lane.period = sigx.period || 1;
          lane.phase = (sigx.phase ? sigx.phase * 2 : 0) + lane.xmin_cfg;
          tmp0[0] = sigx.name || " ";
          tmp0[1] = (sigx.phase || 0) + lane.xmin_cfg / 2;
          let content_wave = null;
          let num_unseen_markers;
          if (typeof sigx.wave === "string") {
            const parsed_wave_lane = parseWaveLane(sigx.wave, lane.period * lane.hscale - 1, lane);
            content_wave = parsed_wave_lane[0];
            num_unseen_markers = parsed_wave_lane[1];
          }
          current.push(
            tmp0.slice(0),
            content_wave,
            data_extract(sigx, num_unseen_markers),
            sigx
          );
        });
        return content;
      }
      module2.exports = parseWaveLanes;
    }
  });

  // node_modules/onml/tt.js
  var require_tt = __commonJS({
    "node_modules/onml/tt.js"(exports2, module2) {
      "use strict";
      module2.exports = (x, y, obj) => {
        let objt = {};
        if (x || y) {
          const tt = [x || 0].concat(y ? [y] : []);
          objt = { transform: "translate(" + tt.join(",") + ")" };
        }
        obj = typeof obj === "object" ? obj : {};
        return Object.assign(objt, obj);
      };
    }
  });

  // lib/render-groups.js
  var require_render_groups = __commonJS({
    "lib/render-groups.js"(exports2, module2) {
      "use strict";
      var tspan = require_lib();
      var tt = require_tt();
      function renderGroups(groups, index, lane) {
        const res = ["g"];
        groups.map((e, i) => {
          res.push([
            "path",
            {
              id: "group_" + i + "_" + index,
              d: "m " + (e.x + 0.5) + "," + (e.y * lane.yo + 3.5 + lane.yh0 + lane.yh1) + " c -3,0 -5,2 -5,5 l 0," + (e.height * lane.yo - 16) + " c 0,3 2,5 5,5",
              style: "stroke:#0041c4;stroke-width:1;fill:none"
            }
          ]);
          if (e.name === void 0) {
            return;
          }
          const x = e.x - 10;
          const y = lane.yo * (e.y + e.height / 2) + lane.yh0 + lane.yh1;
          const ts = tspan.parse(e.name);
          res.push([
            "g",
            tt(x, y),
            [
              "g",
              { transform: "rotate(270)" },
              ["text", {
                "text-anchor": "middle",
                class: "info",
                "xml:space": "preserve"
              }].concat(ts)
            ]
          ]);
        });
        return res;
      }
      module2.exports = renderGroups;
    }
  });

  // lib/render-marks.js
  var require_render_marks = __commonJS({
    "lib/render-marks.js"(exports2, module2) {
      "use strict";
      var tspan = require_lib();
      function captext(cxt, anchor, y) {
        if (cxt[anchor] && cxt[anchor].text) {
          return [
            ["text", {
              x: cxt.xmax * cxt.xs / 2,
              y,
              fill: "#000",
              "text-anchor": "middle",
              "xml:space": "preserve"
            }].concat(tspan.parse(cxt[anchor].text))
          ];
        }
        return [];
      }
      function ticktock(cxt, ref1, ref2, x, dx, y, len) {
        let offset;
        let L = [];
        if (cxt[ref1] === void 0 || cxt[ref1][ref2] === void 0) {
          return [];
        }
        let val = cxt[ref1][ref2];
        if (typeof val === "string") {
          val = val.trim().split(/\s+/);
        } else if (typeof val === "number" || typeof val === "boolean") {
          offset = Number(val);
          val = [];
          for (let i = 0; i < len; i += 1) {
            val.push(i + offset);
          }
        }
        if (Array.isArray(val)) {
          if (val.length === 0) {
            return [];
          } else if (val.length === 1) {
            offset = Number(val[0]);
            if (isNaN(offset)) {
              L = val;
            } else {
              for (let i = 0; i < len; i += 1) {
                L[i] = i + offset;
              }
            }
          } else if (val.length === 2) {
            offset = Number(val[0]);
            const step = Number(val[1]);
            const tmp = val[1].split(".");
            let dp = 0;
            if (tmp.length === 2) {
              dp = tmp[1].length;
            }
            if (isNaN(offset) || isNaN(step)) {
              L = val;
            } else {
              offset = step * offset;
              for (let i = 0; i < len; i += 1) {
                L[i] = (step * i + offset).toFixed(dp);
              }
            }
          } else {
            L = val;
          }
        } else {
          return [];
        }
        const res = ["g", {
          class: "muted",
          "text-anchor": "middle",
          "xml:space": "preserve"
        }];
        for (let i = 0; i < len; i += 1) {
          if (cxt[ref1] && cxt[ref1].every && (i + offset) % cxt[ref1].every != 0) {
            continue;
          }
          res.push(["text", { x: i * dx + x, y }].concat(tspan.parse(L[i])));
        }
        return [res];
      }
      function renderMarks(content, index, lane, source2) {
        const mstep = 2 * lane.hscale;
        const mmstep = mstep * lane.xs;
        const marks = lane.xmax / mstep;
        const gy = content.length * lane.yo;
        const res = ["g", { id: "gmarks_" + index }];
        const gmarkLines = ["g", { style: "stroke:#888;stroke-width:0.5;stroke-dasharray:1,3" }];
        if (!(source2 && source2.config && source2.config.marks === false)) {
          for (let i = 0; i < marks + 1; i += 1) {
            gmarkLines.push(["line", {
              id: "gmark_" + i + "_" + index,
              x1: i * mmstep,
              y1: 0,
              x2: i * mmstep,
              y2: gy
            }]);
          }
          res.push(gmarkLines);
        }
        return res.concat(
          captext(lane, "head", lane.yh0 ? -33 : -13),
          captext(lane, "foot", gy + (lane.yf0 ? 45 : 25)),
          ticktock(lane, "head", "tick", 0, mmstep, -5, marks + 1),
          ticktock(lane, "head", "tock", mmstep / 2, mmstep, -5, marks),
          ticktock(lane, "foot", "tick", 0, mmstep, gy + 15, marks + 1),
          ticktock(lane, "foot", "tock", mmstep / 2, mmstep, gy + 15, marks)
        );
      }
      module2.exports = renderMarks;
    }
  });

  // lib/arc-shape.js
  var require_arc_shape = __commonJS({
    "lib/arc-shape.js"(exports2, module2) {
      "use strict";
      function arcShape(Edge, from, to) {
        const dx = to.x - from.x;
        const dy = to.y - from.y;
        let lx = (from.x + to.x) / 2;
        const ly = (from.y + to.y) / 2;
        let d;
        let style;
        switch (Edge.shape) {
          case "-": {
            break;
          }
          case "~": {
            d = "M " + from.x + "," + from.y + " c " + 0.7 * dx + ", 0 " + 0.3 * dx + ", " + dy + " " + dx + ", " + dy;
            break;
          }
          case "-~": {
            d = "M " + from.x + "," + from.y + " c " + 0.7 * dx + ", 0 " + dx + ", " + dy + " " + dx + ", " + dy;
            if (Edge.label) {
              lx = from.x + (to.x - from.x) * 0.75;
            }
            break;
          }
          case "~-": {
            d = "M " + from.x + "," + from.y + " c 0, 0 " + 0.3 * dx + ", " + dy + " " + dx + ", " + dy;
            if (Edge.label) {
              lx = from.x + (to.x - from.x) * 0.25;
            }
            break;
          }
          case "-|": {
            d = "m " + from.x + "," + from.y + " " + dx + ",0 0," + dy;
            if (Edge.label) {
              lx = to.x;
            }
            break;
          }
          case "|-": {
            d = "m " + from.x + "," + from.y + " 0," + dy + " " + dx + ",0";
            if (Edge.label) {
              lx = from.x;
            }
            break;
          }
          case "-|-": {
            d = "m " + from.x + "," + from.y + " " + dx / 2 + ",0 0," + dy + " " + dx / 2 + ",0";
            break;
          }
          case "->": {
            style = "marker-end:url(#arrowhead);stroke:#0041c4;stroke-width:1;fill:none";
            break;
          }
          case "~>": {
            style = "marker-end:url(#arrowhead);stroke:#0041c4;stroke-width:1;fill:none";
            d = "M " + from.x + "," + from.y + " c " + 0.7 * dx + ", 0 " + 0.3 * dx + ", " + dy + " " + dx + ", " + dy;
            break;
          }
          case "-~>": {
            style = "marker-end:url(#arrowhead);stroke:#0041c4;stroke-width:1;fill:none";
            d = "M " + from.x + "," + from.y + " c " + 0.7 * dx + ", 0 " + dx + ", " + dy + " " + dx + ", " + dy;
            if (Edge.label) {
              lx = from.x + (to.x - from.x) * 0.75;
            }
            break;
          }
          case "~->": {
            style = "marker-end:url(#arrowhead);stroke:#0041c4;stroke-width:1;fill:none";
            d = "M " + from.x + "," + from.y + " c 0, 0 " + 0.3 * dx + ", " + dy + " " + dx + ", " + dy;
            if (Edge.label) {
              lx = from.x + (to.x - from.x) * 0.25;
            }
            break;
          }
          case "-|>": {
            style = "marker-end:url(#arrowhead);stroke:#0041c4;stroke-width:1;fill:none";
            d = "m " + from.x + "," + from.y + " " + dx + ",0 0," + dy;
            if (Edge.label) {
              lx = to.x;
            }
            break;
          }
          case "|->": {
            style = "marker-end:url(#arrowhead);stroke:#0041c4;stroke-width:1;fill:none";
            d = "m " + from.x + "," + from.y + " 0," + dy + " " + dx + ",0";
            if (Edge.label) {
              lx = from.x;
            }
            break;
          }
          case "-|->": {
            style = "marker-end:url(#arrowhead);stroke:#0041c4;stroke-width:1;fill:none";
            d = "m " + from.x + "," + from.y + " " + dx / 2 + ",0 0," + dy + " " + dx / 2 + ",0";
            break;
          }
          case "<->": {
            style = "marker-end:url(#arrowhead);marker-start:url(#arrowtail);stroke:#0041c4;stroke-width:1;fill:none";
            break;
          }
          case "<~>": {
            style = "marker-end:url(#arrowhead);marker-start:url(#arrowtail);stroke:#0041c4;stroke-width:1;fill:none";
            d = "M " + from.x + "," + from.y + " c " + 0.7 * dx + ", 0 " + 0.3 * dx + ", " + dy + " " + dx + ", " + dy;
            break;
          }
          case "<-~>": {
            style = "marker-end:url(#arrowhead);marker-start:url(#arrowtail);stroke:#0041c4;stroke-width:1;fill:none";
            d = "M " + from.x + "," + from.y + " c " + 0.7 * dx + ", 0 " + dx + ", " + dy + " " + dx + ", " + dy;
            if (Edge.label) {
              lx = from.x + (to.x - from.x) * 0.75;
            }
            break;
          }
          case "<-|>": {
            style = "marker-end:url(#arrowhead);marker-start:url(#arrowtail);stroke:#0041c4;stroke-width:1;fill:none";
            d = "m " + from.x + "," + from.y + " " + dx + ",0 0," + dy;
            if (Edge.label) {
              lx = to.x;
            }
            break;
          }
          case "<-|->": {
            style = "marker-end:url(#arrowhead);marker-start:url(#arrowtail);stroke:#0041c4;stroke-width:1;fill:none";
            d = "m " + from.x + "," + from.y + " " + dx / 2 + ",0 0," + dy + " " + dx / 2 + ",0";
            break;
          }
          case "+": {
            style = "marker-end:url(#tee);marker-start:url(#tee);fill:none;stroke:#00F;stroke-width:1";
            break;
          }
          default: {
            style = "fill:none;stroke:#F00;stroke-width:1";
          }
        }
        return {
          lx,
          ly,
          d,
          style
        };
      }
      module2.exports = arcShape;
    }
  });

  // lib/char-width.json
  var require_char_width = __commonJS({
    "lib/char-width.json"(exports2, module2) {
      module2.exports = { chars: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 34, 47, 74, 74, 118, 89, 25, 44, 44, 52, 78, 37, 44, 37, 37, 74, 74, 74, 74, 74, 74, 74, 74, 74, 74, 37, 37, 78, 78, 78, 74, 135, 89, 89, 96, 96, 89, 81, 103, 96, 37, 67, 89, 74, 109, 96, 103, 89, 103, 96, 89, 81, 96, 89, 127, 89, 87, 81, 37, 37, 37, 61, 74, 44, 74, 74, 67, 74, 74, 37, 74, 74, 30, 30, 67, 30, 112, 74, 74, 74, 74, 44, 67, 37, 74, 67, 95, 66, 65, 67, 44, 34, 44, 78, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 37, 43, 74, 74, 74, 74, 34, 74, 44, 98, 49, 74, 78, 0, 98, 73, 53, 73, 44, 44, 44, 77, 71, 37, 44, 44, 49, 74, 111, 111, 111, 81, 89, 89, 89, 89, 89, 89, 133, 96, 89, 89, 89, 89, 37, 37, 37, 37, 96, 96, 103, 103, 103, 103, 103, 78, 103, 96, 96, 96, 96, 87, 89, 81, 74, 74, 74, 74, 74, 74, 118, 67, 74, 74, 74, 74, 36, 36, 36, 36, 74, 74, 74, 74, 74, 74, 74, 73, 81, 74, 74, 74, 74, 65, 74, 65, 89, 74, 89, 74, 89, 74, 96, 67, 96, 67, 96, 67, 96, 67, 96, 82, 96, 74, 89, 74, 89, 74, 89, 74, 89, 74, 89, 74, 103, 74, 103, 74, 103, 74, 103, 74, 96, 74, 96, 74, 37, 36, 37, 36, 37, 36, 37, 30, 37, 36, 98, 59, 67, 30, 89, 67, 67, 74, 30, 74, 30, 74, 39, 74, 44, 74, 30, 96, 74, 96, 74, 96, 74, 80, 96, 74, 103, 74, 103, 74, 103, 74, 133, 126, 96, 44, 96, 44, 96, 44, 89, 67, 89, 67, 89, 67, 89, 67, 81, 38, 81, 50, 81, 37, 96, 74, 96, 74, 96, 74, 96, 74, 96, 74, 96, 74, 127, 95, 87, 65, 87, 81, 67, 81, 67, 81, 67, 30, 84, 97, 91, 84, 91, 84, 94, 92, 73, 104, 109, 91, 84, 81, 84, 100, 82, 76, 74, 103, 91, 131, 47, 40, 99, 77, 37, 79, 130, 100, 84, 104, 114, 87, 126, 101, 87, 84, 93, 84, 69, 84, 46, 52, 82, 52, 82, 114, 89, 102, 96, 100, 98, 91, 70, 88, 88, 77, 70, 85, 89, 77, 67, 84, 39, 65, 61, 39, 189, 173, 153, 111, 105, 61, 123, 123, 106, 89, 74, 37, 30, 103, 74, 96, 74, 96, 74, 96, 74, 96, 74, 96, 74, 81, 91, 81, 91, 81, 130, 131, 102, 84, 103, 84, 87, 78, 104, 81, 104, 81, 88, 76, 37, 189, 173, 153, 103, 84, 148, 90, 100, 84, 89, 74, 133, 118, 103, 81], other: 114 };
    }
  });

  // lib/text-width.js
  var require_text_width = __commonJS({
    "lib/text-width.js"(exports2, module2) {
      "use strict";
      var charWidth = require_char_width();
      module2.exports = function(str, size) {
        size = size || 11;
        let width = 0;
        for (let i = 0; i < str.length; i++) {
          const c = str.charCodeAt(i);
          let w = charWidth.chars[c];
          if (w === void 0) {
            w = charWidth.other;
          }
          width += w;
        }
        return width * size / 100;
      };
    }
  });

  // lib/render-label.js
  var require_render_label = __commonJS({
    "lib/render-label.js"(exports2, module2) {
      "use strict";
      var tspan = require_lib();
      var tt = require_tt();
      var textWidth = require_text_width();
      function renderLabel(p, text, fontSize) {
        fontSize = fontSize || 11;
        const w = textWidth(text, fontSize) + 2;
        return [
          "g",
          tt(p.x, p.y),
          ["rect", {
            x: -(w >> 1),
            y: -(fontSize >> 1),
            width: w,
            height: fontSize,
            style: "fill:#FFF;"
          }],
          ["text", {
            "text-anchor": "middle",
            y: Math.round(0.3 * fontSize),
            style: "font-size:" + fontSize + "px;"
          }].concat(tspan.parse(text))
        ];
      }
      module2.exports = renderLabel;
    }
  });

  // lib/render-arcs.js
  var require_render_arcs = __commonJS({
    "lib/render-arcs.js"(exports2, module2) {
      "use strict";
      var arcShape = require_arc_shape();
      var renderLabel = require_render_label();
      var renderArc = (Edge, from, to, shapeProps) => ["path", {
        id: "gmark_" + Edge.from + "_" + Edge.to,
        d: shapeProps.d || "M " + from.x + "," + from.y + " " + to.x + "," + to.y,
        style: shapeProps.style || "fill:none;stroke:#00F;stroke-width:1"
      }];
      var labeler = (lane, Events) => (element, i) => {
        const text = element.node;
        lane.period = element.period ? element.period : 1;
        lane.phase = (element.phase ? element.phase * 2 : 0) + lane.xmin_cfg;
        if (text) {
          const stack = text.split("");
          let pos = 0;
          while (stack.length) {
            const eventname = stack.shift();
            if (eventname !== ".") {
              Events[eventname] = {
                x: lane.xs * (2 * pos * lane.period * lane.hscale - lane.phase) + lane.xlabel,
                y: i * lane.yo + lane.y0 + lane.ys * 0.5
              };
            }
            pos += 1;
          }
        }
      };
      var archer = (res, Events, arcFontSize) => (element) => {
        const words = element.trim().split(/\s+/);
        const Edge = {
          words,
          label: element.substring(words[0].length).substring(1),
          from: words[0].substr(0, 1),
          to: words[0].substr(-1, 1),
          shape: words[0].slice(1, -1)
        };
        const from = Events[Edge.from];
        const to = Events[Edge.to];
        if (from && to) {
          const shapeProps = arcShape(Edge, from, to);
          const lx = shapeProps.lx;
          const ly = shapeProps.ly;
          res.push(renderArc(Edge, from, to, shapeProps));
          if (Edge.label) {
            res.push(renderLabel({ x: lx, y: ly }, Edge.label, arcFontSize));
          }
        }
      };
      function renderArcs(lanes, index, source2, lane) {
        const arcFontSize = source2 && source2.config && source2.config.arcFontSize ? source2.config.arcFontSize : 11;
        const res = ["g", { id: "wavearcs_" + index }];
        const Events = {};
        if (Array.isArray(lanes)) {
          lanes.map(labeler(lane, Events));
          if (Array.isArray(source2.edge)) {
            source2.edge.map(archer(res, Events, arcFontSize));
          }
          Object.keys(Events).map(function(k) {
            if (k === k.toLowerCase()) {
              if (Events[k].x > 0) {
                res.push(renderLabel({
                  x: Events[k].x,
                  y: Events[k].y
                }, k + "", arcFontSize));
              }
            }
          });
        }
        return res;
      }
      module2.exports = renderArcs;
    }
  });

  // lib/render-gaps.js
  var require_render_gaps = __commonJS({
    "lib/render-gaps.js"(exports2, module2) {
      "use strict";
      var tt = require_tt();
      function renderGapUses(text, lane) {
        const res = [];
        const Stack = (text || "").split("");
        let pos = 0;
        let subCycle = false;
        while (Stack.length) {
          let next = Stack.shift();
          if (next === "<") {
            subCycle = true;
            next = Stack.shift();
          }
          if (next === ">") {
            subCycle = false;
            next = Stack.shift();
          }
          if (subCycle) {
            pos += 1;
          } else {
            pos += 2 * lane.period;
          }
          if (next === "|") {
            res.push(["use", tt(
              lane.xs * ((pos - (subCycle ? 0 : lane.period)) * lane.hscale - lane.phase),
              0,
              { "xlink:href": "#gap" }
            )]);
          }
        }
        return res;
      }
      function renderGaps(lanes, index, source2, lane) {
        let res = [];
        if (lanes) {
          const lanesLen = lanes.length;
          const vline = (x) => ["line", {
            x1: x,
            x2: x,
            y2: lanesLen * lane.yo,
            style: "stroke:#000;stroke-width:1px"
          }];
          const lineStyle = "fill:none;stroke:#000;stroke-width:1px";
          const bracket = {
            square: {
              left: ["path", { d: "M  2 0 h -4 v " + (lanesLen * lane.yo - 1) + " h  4", style: lineStyle }],
              right: ["path", { d: "M -2 0 h  4 v " + (lanesLen * lane.yo - 1) + " h -4", style: lineStyle }]
            },
            round: {
              left: ["path", { d: "M  2 0 a 4 4 0 0 0 -4 4 v " + (lanesLen * lane.yo - 9) + " a 4 4 0 0 0  4 4", style: lineStyle }],
              right: ["path", { d: "M -2 0 a 4 4 1 0 1  4 4 v " + (lanesLen * lane.yo - 9) + " a 4 4 1 0 1 -4 4", style: lineStyle }],
              rightLeft: ["path", {
                d: "M -5 0 a 4 4 1 0 1  4 4 v " + (lanesLen * lane.yo - 9) + " a 4 4 1 0 1 -4 4M  5 0 a 4 4 0 0 0 -4 4 v " + (lanesLen * lane.yo - 9) + " a 4 4 0 0 0  4 4",
                style: lineStyle
              }],
              leftLeft: ["path", {
                d: "M  2 0 a 4 4 0 0 0 -4 4 v " + (lanesLen * lane.yo - 9) + " a 4 4 0 0 0  4 4M  5 1 a 3 3 0 0 0 -3 3 v " + (lanesLen * lane.yo - 9) + " a 3 3 0 0 0  3 3",
                style: lineStyle
              }],
              rightRight: ["path", {
                d: "M -5 1 a 3 3 1 0 1  3 3 v " + (lanesLen * lane.yo - 9) + " a 3 3 1 0 1 -3 3M -2 0 a 4 4 1 0 1  4 4 v " + (lanesLen * lane.yo - 9) + " a 4 4 1 0 1 -4 4",
                style: lineStyle
              }]
            }
          };
          const backDrop = (w) => ["rect", {
            x: -w / 2,
            width: w,
            height: lanesLen * lane.yo,
            style: "fill:#ffffffcc;stroke:none"
          }];
          if (source2 && typeof source2.gaps === "string") {
            const scale = lane.hscale * lane.xs * 2;
            const gaps = source2.gaps.trim().split(/\s+/);
            for (let x = 0; x < gaps.length; x++) {
              const c = gaps[x];
              if (c.match(/^[.]$/)) {
                continue;
              }
              const offset = c === c.toLowerCase() ? 0.5 : 0;
              let marks = [];
              switch (c) {
                case "0":
                  marks = [backDrop(4)];
                  break;
                case "1":
                  marks = [backDrop(4), vline(0)];
                  break;
                case "|":
                  marks = [backDrop(4), vline(0)];
                  break;
                case "2":
                  marks = [backDrop(4), vline(-2), vline(2)];
                  break;
                case "3":
                  marks = [backDrop(6), vline(-3), vline(0), vline(3)];
                  break;
                case "[":
                  marks = [backDrop(4), bracket.square.left];
                  break;
                case "]":
                  marks = [backDrop(4), bracket.square.right];
                  break;
                case "(":
                  marks = [backDrop(4), bracket.round.left];
                  break;
                case ")":
                  marks = [backDrop(4), bracket.round.right];
                  break;
                case ")(":
                  marks = [backDrop(8), bracket.round.rightLeft];
                  break;
                case "((":
                  marks = [backDrop(8), bracket.round.leftLeft];
                  break;
                case "))":
                  marks = [backDrop(8), bracket.round.rightRight];
                  break;
                case "s":
                  for (let idx = 0; idx < lanesLen; idx++) {
                    if (lanes[idx] && lanes[idx].wave && lanes[idx].wave.length > x) {
                      marks.push(["use", tt(2, 5 + lane.yo * idx, { "xlink:href": "#gap" })]);
                    }
                  }
                  break;
              }
              res.push(["g", tt(scale * (x + offset))].concat(marks));
            }
          }
          for (let idx = 0; idx < lanesLen; idx++) {
            const val = lanes[idx];
            lane.period = val.period ? val.period : 1;
            lane.phase = (val.phase ? val.phase * 2 : 0) + lane.xmin_cfg;
            if (typeof val.wave === "string") {
              const gaps = renderGapUses(val.wave, lane);
              res = res.concat([["g", tt(
                0,
                lane.y0 + idx * lane.yo,
                { id: "wavegap_" + idx + "_" + index }
              )].concat(gaps)]);
            }
          }
        }
        return ["g", { id: "wavegaps_" + index }].concat(res);
      }
      module2.exports = renderGaps;
    }
  });

  // lib/render-piece-wise.js
  var require_render_piece_wise = __commonJS({
    "lib/render-piece-wise.js"(exports2, module2) {
      "use strict";
      var tt = require_tt();
      var scaled = (d, sx, sy) => {
        if (sy === void 0) {
          sy = sx;
        }
        let i = 0;
        while (i < d.length) {
          switch (d[i].toLowerCase()) {
            case "h":
              while (i < d.length && !isNaN(d[i + 1])) {
                d[i + 1] *= sx;
                i++;
              }
              break;
            case "v":
              while (i < d.length && !isNaN(d[i + 1])) {
                d[i + 1] *= sy;
                i++;
              }
              break;
            case "m":
            case "l":
            case "t":
              while (i + 1 < d.length && !isNaN(d[i + 1])) {
                d[i + 1] *= sx;
                d[i + 2] *= sy;
                i += 2;
              }
              break;
            case "q":
              while (i + 3 < d.length && !isNaN(d[i + 1])) {
                d[i + 1] *= sx;
                d[i + 2] *= sy;
                d[i + 3] *= sx;
                d[i + 4] *= sy;
                i += 4;
              }
              break;
            case "a":
              while (i + 6 < d.length && !isNaN(d[i + 1])) {
                d[i + 1] *= sx;
                d[i + 2] *= sy;
                d[i + 6] *= sx;
                d[i + 7] *= sy;
                i += 7;
              }
              break;
          }
          i++;
        }
        return d;
      };
      function scale(d, cfg) {
        if (typeof d === "string") {
          d = d.trim().split(/[\s,]+/);
        }
        if (!Array.isArray(d)) {
          return;
        }
        return scaled(d, 2 * cfg.xs, -cfg.ys);
      }
      function renderLane(wave, idx, cfg) {
        if (Array.isArray(wave)) {
          const tag = wave[0];
          const attr = wave[1];
          if (tag === "pw" && typeof attr === "object") {
            const d = scale(attr.d, cfg);
            return [
              "g",
              tt(0, cfg.yo * idx + cfg.ys + cfg.y0),
              ["path", { style: "fill:none;stroke:#000;stroke-width:1px;", d }]
            ];
          }
        }
      }
      function renderPieceWise(lanes, index, cfg) {
        let res = ["g"];
        lanes.map((row, idx) => {
          const wave = row.wave;
          if (Array.isArray(wave)) {
            res.push(renderLane(wave, idx, cfg));
          }
        });
        return res;
      }
      module2.exports = renderPieceWise;
    }
  });

  // lib/render-lanes.js
  var require_render_lanes = __commonJS({
    "lib/render-lanes.js"(exports2, module2) {
      "use strict";
      var renderMarks = require_render_marks();
      var renderArcs = require_render_arcs();
      var renderGaps = require_render_gaps();
      var renderPieceWise = require_render_piece_wise();
      function renderLanes(index, content, waveLanes, ret, source2, lane) {
        return [
          renderMarks(content, index, lane, source2)
        ].concat(
          waveLanes.res,
          [
            renderArcs(ret.lanes, index, source2, lane),
            renderGaps(ret.lanes, index, source2, lane),
            renderPieceWise(ret.lanes, index, lane)
          ]
        );
      }
      module2.exports = renderLanes;
    }
  });

  // lib/render-over-under.js
  var require_render_over_under = __commonJS({
    "lib/render-over-under.js"(exports2, module2) {
      "use strict";
      var tt = require_tt();
      var colors = {
        1: "#000000",
        2: "#e90000",
        3: "#3edd00",
        4: "#0074cd",
        5: "#ff15db",
        6: "#af9800",
        7: "#00864f",
        8: "#a076ff"
      };
      function renderOverUnder(el, key, lane) {
        const xs = lane.xs;
        const ys = lane.ys;
        const period = (el.period || 1) * 2 * xs;
        const xoffset = -(el.phase || 0) * 2 * xs;
        const gap1 = 12;
        const serif = 7;
        let color;
        const y = key === "under" ? ys : 0;
        let start;
        function line(x) {
          return start === void 0 ? [] : [["line", {
            style: "stroke:" + color,
            x1: period * start + gap1,
            x2: period * x
          }]];
        }
        if (el[key]) {
          let res = ["g", tt(
            xoffset,
            y,
            { style: "stroke-width:3" }
          )];
          const arr = el[key].split("");
          arr.map(function(dot, i) {
            if (dot !== "." && start !== void 0) {
              res = res.concat(line(i));
              if (key === "over") {
                res.push(["path", {
                  style: "stroke:none;fill:" + color,
                  d: "m" + (period * i - serif) + " 0 l" + serif + " " + serif + " v-" + serif + " z"
                }]);
              }
            }
            if (dot === "0") {
              start = void 0;
            } else if (dot !== ".") {
              start = i;
              color = colors[dot] || colors[1];
            }
          });
          if (start !== void 0) {
            res = res.concat(line(arr.length));
          }
          return [res];
        }
        return [];
      }
      module2.exports = renderOverUnder;
    }
  });

  // lib/render-wave-lane.js
  var require_render_wave_lane = __commonJS({
    "lib/render-wave-lane.js"(exports2, module2) {
      "use strict";
      var tt = require_tt();
      var tspan = require_lib();
      var textWidth = require_text_width();
      var findLaneMarkers = require_find_lane_markers();
      var renderOverUnder = require_render_over_under();
      function renderLaneUses(cont, lane) {
        const res = [];
        if (cont[1]) {
          cont[1].map(function(ref, i) {
            res.push(["use", tt(i * lane.xs, 0, { "xlink:href": "#" + ref })]);
          });
          if (cont[2] && cont[2].length) {
            const labels = findLaneMarkers(cont[1]);
            if (labels.length) {
              labels.map(function(label, i) {
                if (cont[2] && cont[2][i] !== void 0) {
                  res.push(["text", {
                    x: label * lane.xs + lane.xlabel,
                    y: lane.ym,
                    "text-anchor": "middle",
                    "xml:space": "preserve"
                  }].concat(tspan.parse(cont[2][i])));
                }
              });
            }
          }
        }
        return res;
      }
      function renderWaveLane(content, index, lane) {
        let xmax = 0;
        const glengths = [];
        const res = [];
        content.map(function(el, j) {
          const name = el[0][0];
          if (name) {
            let xoffset = el[0][1];
            xoffset = xoffset > 0 ? Math.ceil(2 * xoffset) - 2 * xoffset : -2 * xoffset;
            res.push(
              ["g", tt(
                0,
                lane.y0 + j * lane.yo,
                { id: "wavelane_" + j + "_" + index }
              )].concat([
                ["text", {
                  x: lane.tgo,
                  y: lane.ym,
                  class: "info",
                  "text-anchor": "end",
                  "xml:space": "preserve"
                }].concat(tspan.parse(name))
              ]).concat([
                ["g", tt(
                  xoffset * lane.xs,
                  0,
                  { id: "wavelane_draw_" + j + "_" + index }
                )].concat(renderLaneUses(el, lane))
              ]).concat(
                renderOverUnder(el[3], "over", lane),
                renderOverUnder(el[3], "under", lane)
              )
            );
            xmax = Math.max(xmax, (el[1] || []).length);
            glengths.push(name.textWidth ? name.textWidth : name.charCodeAt ? textWidth(name, 11) : 0);
          }
        });
        lane.xmax = Math.min(xmax, lane.xmax_cfg - lane.xmin_cfg);
        const xgmax = 0;
        lane.xg = xgmax + 20;
        return { glengths, res };
      }
      module2.exports = renderWaveLane;
    }
  });

  // lib/w3.js
  var require_w3 = __commonJS({
    "lib/w3.js"(exports2, module2) {
      "use strict";
      module2.exports = {
        svg: "http://www.w3.org/2000/svg",
        xlink: "http://www.w3.org/1999/xlink",
        xmlns: "http://www.w3.org/XML/1998/namespace"
      };
    }
  });

  // lib/insert-svg-template.js
  var require_insert_svg_template = __commonJS({
    "lib/insert-svg-template.js"(exports2, module2) {
      "use strict";
      var tt = require_tt();
      var w3 = require_w3();
      function insertSVGTemplate(index, source2, lane, waveSkin, content, lanes, groups, notFirstSignal) {
        const waveSkinNames = Object.keys(waveSkin);
        let skin = waveSkin.default || waveSkin[waveSkinNames[0]];
        if (source2 && source2.config && source2.config.skin && waveSkin[source2.config.skin]) {
          skin = waveSkin[source2.config.skin];
        }
        const e = notFirstSignal ? ["svg", { id: "svg", xmlns: w3.svg, "xmlns:xlink": w3.xlink }, ["g"]] : skin;
        const width = lane.xg + lane.xs * (lane.xmax + 1);
        const height = content.length * lane.yo + lane.yh0 + lane.yh1 + lane.yf0 + lane.yf1;
        const body = e[e.length - 1];
        body[1] = { id: "waves_" + index };
        body[2] = ["rect", { width, height, style: "stroke:none;fill:white" }];
        body[3] = ["g", tt(
          lane.xg + 0.5,
          lane.yh0 + lane.yh1 + 0.5,
          { id: "lanes_" + index }
        )].concat(lanes);
        body[4] = ["g", {
          id: "groups_" + index
        }, groups];
        const head = e[1];
        head.id = "svgcontent_" + index;
        head.xmlns = w3.svg;
        head["xmlns:xlink"] = w3.xlink;
        head.height = height;
        head.width = width;
        head.viewBox = "0 0 " + width + " " + height;
        head.overflow = "hidden";
        return e;
      }
      module2.exports = insertSVGTemplate;
    }
  });

  // lib/render-signal.js
  var require_render_signal = __commonJS({
    "lib/render-signal.js"(exports2, module2) {
      "use strict";
      var rec = require_rec();
      var lane = require_lane();
      var parseConfig = require_parse_config();
      var parseWaveLanes = require_parse_wave_lanes();
      var renderGroups = require_render_groups();
      var renderLanes = require_render_lanes();
      var renderWaveLane = require_render_wave_lane();
      var insertSVGTemplate = require_insert_svg_template();
      function laneParamsFromSkin(index, source2, lane2, waveSkin, notFirstSignal) {
        if (notFirstSignal) {
          return;
        }
        const waveSkinNames = Object.keys(waveSkin);
        if (waveSkinNames.length === 0) {
          throw new Error("no skins found");
        }
        let skin = waveSkin.default || waveSkin[waveSkinNames[0]];
        if (source2 && source2.config && source2.config.skin && waveSkin[source2.config.skin]) {
          skin = waveSkin[source2.config.skin];
        }
        const socket = skin[3][1][2][1];
        lane2.xs = Number(socket.width);
        lane2.ys = Number(socket.height);
        lane2.xlabel = Number(socket.x);
        lane2.ym = Number(socket.y);
      }
      function renderSignal(index, source2, waveSkin, notFirstSignal) {
        laneParamsFromSkin(index, source2, lane, waveSkin, notFirstSignal);
        parseConfig(source2, lane);
        const ret = rec(source2.signal, { x: 0, y: 0, xmax: 0, width: [], lanes: [], groups: [] });
        const content = parseWaveLanes(ret.lanes, lane);
        const waveLanes = renderWaveLane(content, index, lane);
        const waveGroups = renderGroups(ret.groups, index, lane);
        const xmax = waveLanes.glengths.reduce((res, len, i) => Math.max(res, len + ret.width[i]), 0);
        lane.xg = Math.ceil((xmax - lane.tgo) / lane.xs) * lane.xs;
        return insertSVGTemplate(
          index,
          source2,
          lane,
          waveSkin,
          content,
          renderLanes(index, content, waveLanes, ret, source2, lane),
          waveGroups,
          notFirstSignal
        );
      }
      module2.exports = renderSignal;
    }
  });

  // lib/render-any.js
  var require_render_any = __commonJS({
    "lib/render-any.js"(exports2, module2) {
      "use strict";
      var renderAssign = require_render_assign();
      var renderReg = require_render_reg();
      var renderSignal = require_render_signal();
      var w3 = require_w3();
      function renderAny(index, source2, waveSkin, notFirstSignal, notFirstAssign) {
        const res = source2.signal ? renderSignal(index, source2, waveSkin, notFirstSignal) : source2.assign ? renderAssign(index, source2, notFirstAssign) : source2.reg ? renderReg(index, source2) : ["div", {}];
        if (res[0] === "svg") {
          res[1].xmlns = w3.svg;
          res[1]["xmlns:xlink"] = w3.xlink;
        }
        res[1].class = "WaveDrom";
        return res;
      }
      module2.exports = renderAny;
    }
  });

  // lib/append-save-as-dialog.js
  var require_append_save_as_dialog = __commonJS({
    "lib/append-save-as-dialog.js"(exports2, module2) {
      "use strict";
      var stringify = require_stringify();
      var eva3 = require_eva();
      var renderAny = require_render_any();
      function b64(str) {
        return btoa(encodeURIComponent(str).replace(
          /%([0-9A-F]{2})/g,
          (_, p1) => String.fromCharCode(parseInt(p1, 16))
        ));
      }
      function appendSaveAsDialog(index, output) {
        let menu;
        function svgString() {
          const src = document.getElementById("InputJSON_" + index);
          if (src) {
            return stringify(renderAny(index, eva3("InputJSON_" + index), window.WaveSkin, false, false));
          }
          return div.innerHTML;
        }
        function closeMenu(e) {
          const left = parseInt(menu.style.left, 10);
          const top = parseInt(menu.style.top, 10);
          if (e.x < left || e.x > left + menu.offsetWidth || e.y < top || e.y > top + menu.offsetHeight) {
            menu.parentNode.removeChild(menu);
            document.body.removeEventListener("mousedown", closeMenu, false);
          }
        }
        const div = document.getElementById(output + index);
        div.childNodes[0].addEventListener(
          "contextmenu",
          function(e) {
            menu = document.createElement("div");
            menu.className = "wavedromMenu";
            menu.style.top = e.y + "px";
            menu.style.left = e.x + "px";
            const list = document.createElement("ul");
            const savePng = document.createElement("li");
            savePng.innerHTML = "Save as PNG";
            list.appendChild(savePng);
            const saveSvg = document.createElement("li");
            saveSvg.innerHTML = "Save as SVG";
            list.appendChild(saveSvg);
            menu.appendChild(list);
            document.body.appendChild(menu);
            savePng.addEventListener(
              "click",
              function() {
                const svgdata = "data:image/svg+xml;base64," + b64(svgString());
                const img = new Image();
                img.src = svgdata;
                img.onload = function() {
                  const canvas = document.createElement("canvas");
                  canvas.width = img.width;
                  canvas.height = img.height;
                  const context = canvas.getContext("2d");
                  context.drawImage(img, 0, 0);
                  const pngdata = canvas.toDataURL("image/png");
                  const a = document.createElement("a");
                  a.href = pngdata;
                  a.download = "wavedrom.png";
                  a.click();
                  menu.parentNode.removeChild(menu);
                  document.body.removeEventListener("mousedown", closeMenu, false);
                };
              },
              false
            );
            saveSvg.addEventListener(
              "click",
              function() {
                const svgdata = "data:image/svg+xml;base64," + b64(svgString());
                const a = document.createElement("a");
                a.href = svgdata;
                a.download = "wavedrom.svg";
                a.click();
                menu.parentNode.removeChild(menu);
                document.body.removeEventListener("mousedown", closeMenu, false);
              },
              false
            );
            menu.addEventListener(
              "contextmenu",
              function(ee) {
                ee.preventDefault();
              },
              false
            );
            document.body.addEventListener("mousedown", closeMenu, false);
            e.preventDefault();
          },
          false
        );
      }
      module2.exports = appendSaveAsDialog;
    }
  });

  // lib/create-element.js
  var require_create_element = __commonJS({
    "lib/create-element.js"(exports2, module2) {
      "use strict";
      var stringify = require_stringify();
      var w3 = require_w3();
      function createElement(arr) {
        arr[1].xmlns = w3.svg;
        arr[1]["xmlns:xlink"] = w3.xlink;
        const s1 = stringify(arr);
        const parser = new DOMParser();
        const doc = parser.parseFromString(s1, "image/svg+xml");
        return doc.firstChild;
      }
      module2.exports = createElement;
    }
  });

  // lib/render-wave-element.js
  var require_render_wave_element = __commonJS({
    "lib/render-wave-element.js"(exports2, module2) {
      "use strict";
      var renderAny = require_render_any();
      var createElement = require_create_element();
      function renderWaveElement(index, source2, outputElement, waveSkin, notFirstSignal, notFirstAssign) {
        while (outputElement.childNodes.length) {
          outputElement.removeChild(outputElement.childNodes[0]);
        }
        outputElement.insertBefore(createElement(
          renderAny(index, source2, waveSkin, notFirstSignal, notFirstAssign)
        ), null);
      }
      module2.exports = renderWaveElement;
    }
  });

  // lib/render-wave-form.js
  var require_render_wave_form = __commonJS({
    "lib/render-wave-form.js"(exports2, module2) {
      "use strict";
      var renderWaveElement = require_render_wave_element();
      function renderWaveForm2(index, source2, output, notFirstSignal, notFirstAssign) {
        renderWaveElement(index, source2, document.getElementById(output + index), window.WaveSkin, notFirstSignal, notFirstAssign);
      }
      module2.exports = renderWaveForm2;
    }
  });

  // lib/process-all.js
  var require_process_all = __commonJS({
    "lib/process-all.js"(exports2, module2) {
      "use strict";
      var eva3 = require_eva();
      var appendSaveAsDialog = require_append_save_as_dialog();
      var renderWaveForm2 = require_render_wave_form();
      function processAll2() {
        let index = 0;
        const points = document.querySelectorAll("*");
        for (let i = 0; i < points.length; i++) {
          if (points.item(i).type && points.item(i).type.toLowerCase() === "wavedrom") {
            points.item(i).setAttribute("id", "InputJSON_" + index);
            const node0 = document.createElement("div");
            node0.id = "WaveDrom_Display_" + index;
            points.item(i).parentNode.insertBefore(node0, points.item(i));
            index += 1;
          }
        }
        let notFirstSignal = false;
        let notFirstAssign = false;
        for (let i = 0; i < index; i += 1) {
          const obj = eva3("InputJSON_" + i);
          renderWaveForm2(i, obj, "WaveDrom_Display_", notFirstSignal, notFirstAssign);
          if (obj && obj.signal) {
            notFirstSignal = true;
          } else if (obj && obj.assign) {
            notFirstAssign = true;
          }
          appendSaveAsDialog(i, "WaveDrom_Display_");
        }
        document.head.insertAdjacentHTML("beforeend", '<style type="text/css">div.wavedromMenu{position:fixed;border:solid 1pt#CCCCCC;background-color:white;box-shadow:0px 10px 20px #808080;cursor:default;margin:0px;padding:0px;}div.wavedromMenu>ul{margin:0px;padding:0px;}div.wavedromMenu>ul>li{padding:2px 10px;list-style:none;}div.wavedromMenu>ul>li:hover{background-color:#b5d5ff;}</style>');
      }
      module2.exports = processAll2;
    }
  });

  // lib/editor-refresh.js
  var require_editor_refresh = __commonJS({
    "lib/editor-refresh.js"(exports2, module2) {
      "use strict";
      var eva3 = require_eva();
      var renderWaveForm2 = require_render_wave_form();
      function editorRefresh2() {
        renderWaveForm2(0, eva3("InputJSON_0"), "WaveDrom_Display_");
      }
      module2.exports = editorRefresh2;
    }
  });

  // lib/wave-drom.js
  window.WaveDrom = window.WaveDrom || {};
  var pkg = require_package();
  var processAll = require_process_all();
  var eva2 = require_eva();
  var renderWaveForm = require_render_wave_form();
  var editorRefresh = require_editor_refresh();
  window.WaveDrom.ProcessAll = processAll;
  window.WaveDrom.RenderWaveForm = renderWaveForm;
  window.WaveDrom.EditorRefresh = editorRefresh;
  window.WaveDrom.eva = eva2;
  window.WaveDrom.version = pkg.version;
})();
