import type { JsonSchemaType, JsonValue, ProposalSchema } from "./types.js";

function isObject(value: JsonValue): value is { [key: string]: JsonValue } {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function equals(left: JsonValue, right: JsonValue): boolean {
  if (left === right) return true;
  if (Array.isArray(left) || Array.isArray(right)) {
    return (
      Array.isArray(left) &&
      Array.isArray(right) &&
      left.length === right.length &&
      left.every((item, index) => equals(item, right[index]!))
    );
  }
  if (!isObject(left) || !isObject(right)) return false;
  const leftKeys = Object.keys(left);
  const rightKeys = Object.keys(right);
  return (
    leftKeys.length === rightKeys.length &&
    leftKeys.every(
      (key) => Object.hasOwn(right, key) && equals(left[key]!, right[key]!),
    )
  );
}

function matchesType(value: JsonValue, type: JsonSchemaType): boolean {
  switch (type) {
    case "null":
      return value === null;
    case "boolean":
      return typeof value === "boolean";
    case "object":
      return isObject(value);
    case "array":
      return Array.isArray(value);
    case "number":
      return typeof value === "number" && Number.isFinite(value);
    case "integer":
      return typeof value === "number" && Number.isInteger(value);
    case "string":
      return typeof value === "string";
  }
}

function childPath(path: string, key: string | number): string {
  return typeof key === "number" ? `${path}[${key}]` : `${path}.${key}`;
}

export function validateSchema(
  value: JsonValue,
  schema: ProposalSchema | boolean,
  path = "$",
): string[] {
  if (schema === true) return [];
  if (schema === false) return [`${path} is forbidden by the schema`];

  const errors: string[] = [];
  const types =
    schema.type === undefined
      ? []
      : Array.isArray(schema.type)
        ? schema.type
        : [schema.type];
  if (types.length > 0 && !types.some((type) => matchesType(value, type))) {
    errors.push(`${path} must have type ${types.join(" or ")}`);
    return errors;
  }

  if (
    schema.enum &&
    !schema.enum.some((candidate) => equals(value, candidate))
  ) {
    errors.push(`${path} must equal one of the allowed values`);
  }
  if (schema.const !== undefined && !equals(value, schema.const)) {
    errors.push(`${path} must equal the constant value`);
  }

  if (isObject(value)) {
    for (const required of schema.required ?? []) {
      if (!Object.hasOwn(value, required))
        errors.push(`${childPath(path, required)} is required`);
    }
    for (const [key, child] of Object.entries(value)) {
      const propertySchema = schema.properties?.[key];
      if (propertySchema !== undefined) {
        errors.push(
          ...validateSchema(child, propertySchema, childPath(path, key)),
        );
      } else if (schema.additionalProperties === false) {
        errors.push(`${childPath(path, key)} is not an allowed property`);
      } else if (typeof schema.additionalProperties === "object") {
        errors.push(
          ...validateSchema(
            child,
            schema.additionalProperties,
            childPath(path, key),
          ),
        );
      }
    }
  }

  if (Array.isArray(value)) {
    if (schema.minItems !== undefined && value.length < schema.minItems) {
      errors.push(`${path} must contain at least ${schema.minItems} items`);
    }
    if (schema.maxItems !== undefined && value.length > schema.maxItems) {
      errors.push(`${path} must contain at most ${schema.maxItems} items`);
    }
    if (
      schema.uniqueItems &&
      value.some((item, index) =>
        value.slice(0, index).some((previous) => equals(item, previous)),
      )
    ) {
      errors.push(`${path} must contain unique items`);
    }
    if (schema.items !== undefined) {
      value.forEach((item, index) => {
        errors.push(
          ...validateSchema(item, schema.items!, childPath(path, index)),
        );
      });
    }
  }

  if (typeof value === "string") {
    if (schema.minLength !== undefined && value.length < schema.minLength) {
      errors.push(`${path} must have a minimum length of ${schema.minLength}`);
    }
    if (schema.maxLength !== undefined && value.length > schema.maxLength) {
      errors.push(`${path} must have a maximum length of ${schema.maxLength}`);
    }
    if (schema.pattern !== undefined) {
      try {
        if (!new RegExp(schema.pattern, "u").test(value)) {
          errors.push(`${path} must match pattern ${schema.pattern}`);
        }
      } catch {
        errors.push(`${path} uses an invalid schema pattern`);
      }
    }
  }

  if (typeof value === "number") {
    if (schema.minimum !== undefined && value < schema.minimum) {
      errors.push(`${path} must be at least the minimum ${schema.minimum}`);
    }
    if (schema.maximum !== undefined && value > schema.maximum) {
      errors.push(`${path} must be at most the maximum ${schema.maximum}`);
    }
    if (
      schema.exclusiveMinimum !== undefined &&
      value <= schema.exclusiveMinimum
    ) {
      errors.push(`${path} must be greater than ${schema.exclusiveMinimum}`);
    }
    if (
      schema.exclusiveMaximum !== undefined &&
      value >= schema.exclusiveMaximum
    ) {
      errors.push(`${path} must be less than ${schema.exclusiveMaximum}`);
    }
  }

  for (const childSchema of schema.allOf ?? []) {
    errors.push(...validateSchema(value, childSchema, path));
  }
  if (
    schema.anyOf &&
    !schema.anyOf.some(
      (child) => validateSchema(value, child, path).length === 0,
    )
  ) {
    errors.push(`${path} must match at least one anyOf schema`);
  }
  if (schema.oneOf) {
    const matches = schema.oneOf.filter(
      (child) => validateSchema(value, child, path).length === 0,
    );
    if (matches.length !== 1)
      errors.push(`${path} must match exactly one oneOf schema`);
  }
  if (
    schema.not !== undefined &&
    validateSchema(value, schema.not, path).length === 0
  ) {
    errors.push(`${path} must not match the excluded schema`);
  }

  return errors;
}
