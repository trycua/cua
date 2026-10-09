// Preserve the pre-enum SDK's boolean inputs at the language lowering boundary.
// The native ABI and advertised Vertex-compatible enum schema remain unchanged.
const fields = ["trail", "glow", "magnet", "ripple", "squish"]
function replaceOnce(source, before, after) {
  if (source.split(before).length !== 2) {
    throw new Error(`unexpected cursor-effect binding template: ${before}`)
  }
  return source.replace(before, after)
}

export function normalizePythonCursorEffects(source) {
  if (!source.includes("class CursorMotionEffects:")) return source
  for (const field of fields) {
    source = replaceOnce(source, `${field}:typing.Optional[CursorEffectSetting]`,
      `${field}:typing.Optional[typing.Union[CursorEffectSetting, bool]]`)
  }
  const start = source.indexOf("class _UniffiFfiConverterTypeCursorEffectSetting(")
  const end = source.indexOf("class _UniffiFfiConverterOptionalTypeCursorEffectSetting(", start)
  if (start < 0 || end < 0) throw new Error("missing cursor effect converters")
  let converter = source.slice(start, end)
  converter = replaceOnce(converter, "    def check_lower(value):\n",
    "    def check_lower(value):\n        if type(value) is bool:\n            return\n")
  converter = replaceOnce(converter, "    def write(value, buf):\n",
    "    def write(value, buf):\n        if type(value) is bool:\n            value = CursorEffectSetting.ON if value else CursorEffectSetting.OFF\n")
  return source.slice(0, start) + converter + source.slice(end)
}

export function normalizeTypeScriptCursorEffects(source) {
  if (!source.includes("export type CursorMotionEffects = {")) return source
  for (const field of fields) {
    source = replaceOnce(source, `${field}?: CursorEffectSetting`,
      `${field}?: CursorEffectSetting | boolean`)
  }
  const start = source.indexOf("const FfiConverterTypeCursorEffectSetting = (() => {")
  const end = source.indexOf("export type CursorMotionEffects = {", start)
  if (start < 0 || end < 0) throw new Error("missing cursor effect converter")
  let converter = source.slice(start, end)
  converter = replaceOnce(converter, "type TypeName = CursorEffectSetting;", "type TypeName = CursorEffectSetting | boolean;")
  converter = replaceOnce(converter, "        write(value: TypeName, into: RustBuffer): void {\n",
    "        write(value: TypeName, into: RustBuffer): void {\n            if (typeof value === \"boolean\") value = value ? CursorEffectSetting.On : CursorEffectSetting.Off;\n")
  converter = replaceOnce(converter, "                case CursorEffectSetting.Default: return ordinalConverter.write(3, into);",
    "                case CursorEffectSetting.Default: return ordinalConverter.write(3, into);\n                default: throw new UniffiInternalError.UnexpectedEnumCase();")
  return source.slice(0, start) + converter + source.slice(end)
}
