export interface PairFormFields {
  name: string;
  leftPath: string;
  rightPath: string;
}

export interface PairValidationContext {
  leftExists: boolean;
  rightExists: boolean;
  pathsEqual: boolean;
}

export function validatePairForm(
  fields: PairFormFields,
  ctx: PairValidationContext,
): string[] {
  const errors: string[] = [];
  const name = fields.name.trim();
  const left = fields.leftPath.trim();
  const right = fields.rightPath.trim();

  if (!name) {
    errors.push("Name is required");
  }
  if (!left) {
    errors.push("Left folder path is required");
  } else if (!ctx.leftExists) {
    errors.push("Left folder does not exist");
  }
  if (!right) {
    errors.push("Right folder path is required");
  } else if (!ctx.rightExists) {
    errors.push("Right folder does not exist");
  }
  if (left && right && ctx.pathsEqual) {
    errors.push("Left and right folders must be different");
  }

  return errors;
}
