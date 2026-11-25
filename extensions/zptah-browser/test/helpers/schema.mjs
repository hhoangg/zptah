// @ts-nocheck - a validator for the JSON Schema subset protocol.json uses (same as the Rust test).
import assert from "node:assert/strict";

function resolve(root, ref) {
  return ref.slice(2).split("/").reduce((n, p) => n[p], root);
}

export function validate(root, schema, value, path = "$") {
  if (schema.$ref) return validate(root, resolve(root, schema.$ref), value, path);
  if (schema.oneOf) {
    if (schema.oneOf.filter((o) => validate(root, o, value, path) === null).length !== 1) return `${path}: oneOf`;
    return null;
  }
  if ("const" in schema && value !== schema.const) return `${path}: const`;
  if (schema.enum && !schema.enum.includes(value)) return `${path}: ${JSON.stringify(value)} not in enum`;
  if (schema.type === "string") {
    if (typeof value !== "string") return `${path}: not a string`;
    if (schema.minLength !== undefined && [...value].length < schema.minLength) return `${path}: too short`;
    if (schema.maxLength !== undefined && [...value].length > schema.maxLength) return `${path}: too long (${[...value].length} > ${schema.maxLength})`;
    if (schema.pattern && !new RegExp(schema.pattern).test(value)) return `${path}: pattern`;
  } else if (schema.type === "integer") {
    if (!Number.isInteger(value)) return `${path}: not an integer`;
    if (schema.minimum !== undefined && value < schema.minimum) return `${path}: below minimum`;
    if (schema.maximum !== undefined && value > schema.maximum) return `${path}: above maximum`;
  } else if (schema.type === "boolean") {
    if (typeof value !== "boolean") return `${path}: not a boolean`;
  } else if (schema.type === "array") {
    if (!Array.isArray(value)) return `${path}: not an array`;
    for (let i = 0; i < value.length; i += 1) {
      const e = schema.items ? validate(root, schema.items, value[i], `${path}[${i}]`) : null;
      if (e) return e;
    }
  } else if (schema.type === "object") {
    if (typeof value !== "object" || value === null || Array.isArray(value)) return `${path}: not an object`;
    for (const k of schema.required ?? []) if (!(k in value)) return `${path}: missing ${k}`;
    for (const [k, sub] of Object.entries(schema.properties ?? {})) {
      if (k in value) {
        const e = validate(root, sub, value[k], `${path}.${k}`);
        if (e) return e;
      }
    }
  }
  return null;
}

/** @param {string} def a name in $defs, or "x-events/<name>" for an event's params */
export function validateAgainst(root, def, value) {
  const schema = def.startsWith("x-events/") ? root["x-events"][def.slice(9)].params : root.$defs[def];
  assert.ok(schema, `no schema ${def}`);
  const e = validate(root, schema, value);
  assert.equal(e, null, `${def}: ${e}\n${JSON.stringify(value, null, 1)}`);
}
