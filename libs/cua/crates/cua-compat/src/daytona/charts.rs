//! Charts for `process.code_run` / `run_code`: matplotlib figures the code
//! leaves open become `artifacts.charts` of the response, the way the SDKs
//! document it ("Matplotlib charts are automatically detected and returned
//! in the `charts` field", `daytona/_sync/process.py` `code_run`;
//! `common/charts.py` for the shapes: `line`, `scatter`, `bar`, `pie`,
//! `box_and_whisker`, `composite_chart`, and a `png` of the figure).
//!
//! Our own mechanism: the Python code runs under a small wrapper that, once
//! the code is done, reads the open figures with matplotlib's public API and
//! prints one marker line with the charts as JSON; the gateway cuts that line
//! out of the output and returns it as `artifacts.charts`. Without matplotlib
//! in the guest (or without figures) `charts` is empty, as with Daytona.

use serde_json::Value;

/// The marker of the line the wrapper prints; never part of user output.
pub const MARKER: &str = "\u{1}\u{2}cua-charts:";

const PRE: &str = r#"
import sys as _s, json as _j, os as _o, builtins as _b
_o.environ.setdefault("MPLBACKEND", "Agg")
import warnings as _w
# plt.show() on the Agg backend only warns that it cannot show: the charts are read instead
_w.filterwarnings("ignore", message="FigureCanvasAgg is non-interactive")
_box = {}
_orig_import = _b.__import__
def _patched_import(name, *a, **k):
    m = _orig_import(name, *a, **k)
    ax_mod = _s.modules.get("matplotlib.axes._axes")
    # only once the module has finished loading (it is imported while it runs)
    if ax_mod is not None and hasattr(ax_mod, "Axes") and not getattr(ax_mod, "_cua_patched", False):
        ax_mod._cua_patched = True
        _bxp = ax_mod.Axes.bxp
        def bxp(self, bxpstats, *aa, **kk):
            _box.setdefault(id(self), []).extend(bxpstats)
            return _bxp(self, bxpstats, *aa, **kk)
        ax_mod.Axes.bxp = bxp
    return m
_b.__import__ = _patched_import
def _cua_charts():
    plt = _s.modules.get("matplotlib.pyplot")
    if plt is None:
        return []
    import base64, io
    def num(v):
        try:
            f = float(v)
        except Exception:
            return None
        return int(f) if f.is_integer() and abs(f) < 1e15 else f
    def lab(a):
        t = a.get_label() if hasattr(a, "get_label") else None
        return None if not t or str(t).startswith("_") else str(t)
    def one(ax):
        d = {"title": ax.get_title() or None, "x_label": ax.get_xlabel() or None, "y_label": ax.get_ylabel() or None}
        try:
            d["x_scale"], d["y_scale"] = ax.get_xscale(), ax.get_yscale()
            d["x_ticks"] = [num(t) for t in ax.get_xticks()]
            d["x_tick_labels"] = [t.get_text() for t in ax.get_xticklabels()]
            d["y_ticks"] = [num(t) for t in ax.get_yticks()]
            d["y_tick_labels"] = [t.get_text() for t in ax.get_yticklabels()]
        except Exception:
            pass
        wedges = [p for p in ax.patches if type(p).__name__ == "Wedge"]
        rects = [p for p in ax.patches if type(p).__name__ == "Rectangle" and p.get_width() is not None]
        colls = [c for c in ax.collections if type(c).__name__ == "PathCollection"]
        lines = [l for l in ax.get_lines()]
        if id(ax) in _box:
            d["type"] = "box_and_whisker"
            d["elements"] = [{"label": str(b.get("label", "")) or None, "min": num(b["whislo"]), "first_quartile": num(b["q1"]),
                              "median": num(b["med"]), "third_quartile": num(b["q3"]), "max": num(b["whishi"]),
                              "outliers": [num(x) for x in b.get("fliers", [])]} for b in _box[id(ax)]]
        elif wedges:
            d["type"] = "pie"
            d["elements"] = [{"label": lab(w) or "", "angle": num(w.theta2 - w.theta1), "radius": num(w.r)} for w in wedges]
        elif rects and len(rects) > 0 and not lines:
            d["type"] = "bar"
            labels = [t.get_text() for t in ax.get_xticklabels()]
            ticks = list(ax.get_xticks())
            els = []
            for r in rects:
                cx = r.get_x() + r.get_width() / 2
                tl = None
                for t, l in zip(ticks, labels):
                    if abs(t - cx) < 1e-9 * max(1, abs(cx)):
                        tl = l
                els.append({"label": tl if tl is not None else (lab(r) or str(num(cx))), "group": lab(r) or "", "value": str(num(r.get_height()))})
            d["elements"] = els
        elif colls and not lines:
            d["type"] = "scatter"
            d["elements"] = [{"label": lab(c) or "", "points": [[num(x), num(y)] for x, y in c.get_offsets()]} for c in colls]
        elif lines:
            d["type"] = "line"
            d["elements"] = [{"label": lab(l) or "", "points": [[num(x), num(y)] for x, y in zip(l.get_xdata(), l.get_ydata())]} for l in lines]
        else:
            d["type"] = "unknown"
            d["elements"] = []
        return d
    out = []
    for n in plt.get_fignums():
        fig = plt.figure(n)
        axes = fig.get_axes()
        if not axes:
            continue
        if len(axes) == 1:
            c = one(axes[0])
        else:
            c = {"type": "composite_chart", "title": (fig._suptitle.get_text() if getattr(fig, "_suptitle", None) else None) or None, "elements": [one(a) for a in axes]}
        try:
            buf = io.BytesIO()
            fig.savefig(buf, format="png")
            c["png"] = base64.b64encode(buf.getvalue()).decode()
        except Exception:
            pass
        out.append({k: v for k, v in c.items() if v is not None})
    return out
_cua_g = {"__name__": "__main__", "__builtins__": _b}
_cua_rc = 0
try:
    exec(compile(_CUA_CODE, "<code>", "exec"), _cua_g)
except SystemExit as _e:
    _cua_rc = _e.code if isinstance(_e.code, int) else (0 if _e.code is None else 1)
    if not isinstance(_e.code, (int, type(None))):
        print(_e.code, file=_s.stderr)
except BaseException:
    import traceback
    traceback.print_exc()
    _cua_rc = 1
finally:
    try:
        _ch = _cua_charts()
        _s.stdout.flush()
        if _ch:
            _o.write(1, ("\n" + MARKER + _j.dumps(_ch) + "\n").encode())
    except Exception:
        pass
_o._exit(_cua_rc) if False else _s.exit(_cua_rc)
"#;

/// The program that runs `code` and reports its charts.
pub fn wrap(code: &str) -> String {
    let lit = serde_json::to_string(code).expect("string");
    let mut s = String::new();
    s.push_str(&format!(
        "_CUA_CODE = {lit}\nMARKER = {}\n",
        serde_json::to_string(MARKER).expect("marker")
    ));
    s.push_str(PRE);
    s
}

/// Cuts the charts line out of `out`: the output without it, and the charts.
pub fn split(out: &str) -> (String, Vec<Value>) {
    match out.find(MARKER) {
        None => (out.to_string(), vec![]),
        Some(i) => {
            let line_end = out[i..].find('\n').map_or(out.len(), |e| i + e);
            let json = &out[i + MARKER.len()..line_end];
            let charts = serde_json::from_str::<Vec<Value>>(json).unwrap_or_default();
            // The wrapper wrote "\n" before the marker and after the line.
            let before = out[..i].strip_suffix('\n').unwrap_or(&out[..i]);
            let after = out.get(line_end + 1..).unwrap_or("");
            (format!("{before}{after}"), charts)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn the_charts_line_is_cut_out() {
        let raw = format!("hello\n\n{MARKER}[{{\"type\":\"line\"}}]\n");
        let (out, charts) = split(&raw);
        assert_eq!(out, "hello\n");
        assert_eq!(charts, vec![serde_json::json!({"type": "line"})]);
        let (out, charts) = split("plain\n");
        assert_eq!((out.as_str(), charts.len()), ("plain\n", 0));
    }

    #[test]
    fn the_wrapper_carries_the_code_verbatim() {
        let code = "print(\"a\\n\")\nx = '''q'''";
        let w = wrap(code);
        let lit = w
            .lines()
            .next()
            .unwrap()
            .strip_prefix("_CUA_CODE = ")
            .unwrap();
        let back: String = serde_json::from_str(lit).unwrap();
        assert_eq!(back, code);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn user_output_survives_the_split(s in "[ -~\n]{0,200}") {
            prop_assume!(!s.contains('\u{1}'));
            let raw = format!("{s}\n{MARKER}[]\n");
            let (out, charts) = split(&raw);
            prop_assert!(charts.is_empty());
            prop_assert_eq!(out, s);
        }

        #[test]
        fn any_code_is_embedded_losslessly(code in "\\PC{0,200}") {
            let w = wrap(&code);
            let lit = w.lines().next().unwrap().strip_prefix("_CUA_CODE = ").unwrap();
            let back: String = serde_json::from_str(lit).unwrap();
            prop_assert_eq!(back, code);
        }
    }
}
