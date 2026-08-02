const FIELD_RANGES = [
  { name: "minute", min: 0, max: 59 },
  { name: "hour", min: 0, max: 23 },
  { name: "day of month", min: 1, max: 31 },
  { name: "month", min: 1, max: 12 },
  { name: "day of week", min: 0, max: 7 },
] as const;

const WEEKDAYS = [
  "Sunday",
  "Monday",
  "Tuesday",
  "Wednesday",
  "Thursday",
  "Friday",
  "Saturday",
  "Sunday",
] as const;

const MONTHS = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
] as const;

export interface CronValidationResult {
  valid: boolean;
  error?: string;
}

function parseFieldToken(
  token: string,
  min: number,
  max: number,
  fieldName: string,
): string | null {
  if (token === "*") {
    return null;
  }

  if (token.includes("/")) {
    const [base, stepRaw] = token.split("/");
    const step = Number(stepRaw);
    if (!Number.isInteger(step) || step <= 0) {
      return `Invalid step in ${fieldName} field`;
    }
    if (base !== "*") {
      const baseError = parseFieldToken(base, min, max, fieldName);
      if (baseError) {
        return baseError;
      }
    }
    return null;
  }

  for (const part of token.split(",")) {
    if (part.includes("-")) {
      const [startRaw, endRaw] = part.split("-");
      const start = Number(startRaw);
      const end = Number(endRaw);
      if (
        !Number.isInteger(start) ||
        !Number.isInteger(end) ||
        start < min ||
        end > max ||
        start > end
      ) {
        return `Invalid range in ${fieldName} field`;
      }
      continue;
    }

    const value = Number(part);
    if (!Number.isInteger(value) || value < min || value > max) {
      return `Invalid value in ${fieldName} field`;
    }
  }

  return null;
}

export function validateCronExpression(
  expression: string,
): CronValidationResult {
  const trimmed = expression.trim();
  if (!trimmed) {
    return { valid: false, error: "Cron expression is required" };
  }

  const fields = trimmed.split(/\s+/);
  if (fields.length !== 5) {
    return {
      valid: false,
      error:
        "Cron expression must have exactly 5 fields (minute hour day month weekday)",
    };
  }

  for (let index = 0; index < fields.length; index += 1) {
    const field = FIELD_RANGES[index];
    const error = parseFieldToken(
      fields[index],
      field.min,
      field.max,
      field.name,
    );
    if (error) {
      return { valid: false, error };
    }
  }

  return { valid: true };
}

function formatHourMinute(hourToken: string, minuteToken: string): string {
  const hour = hourToken === "*" ? 0 : Number(hourToken);
  const minute = minuteToken === "*" ? 0 : Number(minuteToken);
  const period = hour >= 12 ? "PM" : "AM";
  const hour12 = hour % 12 === 0 ? 12 : hour % 12;
  const minuteText = minute.toString().padStart(2, "0");
  return `${hour12}:${minuteText} ${period}`;
}

function describeField(token: string, everyLabel: string): string | null {
  if (token === "*") {
    return everyLabel;
  }
  if (token.startsWith("*/")) {
    const step = token.slice(2);
    return `every ${step}`;
  }
  if (token.includes(",")) {
    return token;
  }
  if (token.includes("-")) {
    return token;
  }
  return token;
}

export function describeCronExpression(expression: string): string {
  const validation = validateCronExpression(expression);
  if (!validation.valid) {
    return validation.error ?? "Invalid cron expression";
  }

  const [minute, hour, day, month, weekday] = expression.trim().split(/\s+/);

  if (
    minute.startsWith("*/") &&
    hour === "*" &&
    day === "*" &&
    month === "*" &&
    weekday === "*"
  ) {
    return `Every ${minute.slice(2)} minutes`;
  }

  if (hour === "*" && day === "*" && month === "*" && weekday === "*") {
    const minuteLabel = describeField(minute, "every minute");
    return `Every hour at minute ${minuteLabel}`;
  }

  if (day === "*" && month === "*" && weekday === "*") {
    return `Every day at ${formatHourMinute(hour, minute)}`;
  }

  if (day === "*" && month === "*" && weekday !== "*") {
    const dayIndex = Number(weekday);
    const dayName =
      Number.isInteger(dayIndex) && dayIndex >= 0 && dayIndex <= 7
        ? WEEKDAYS[dayIndex]
        : `weekday ${weekday}`;
    return `Every ${dayName} at ${formatHourMinute(hour, minute)}`;
  }

  if (month !== "*" && day !== "*") {
    const monthIndex = Number(month);
    const monthName =
      Number.isInteger(monthIndex) && monthIndex >= 1 && monthIndex <= 12
        ? MONTHS[monthIndex - 1]
        : `month ${month}`;
    return `At ${formatHourMinute(hour, minute)} on day ${day} of ${monthName}`;
  }

  const parts = [
    describeField(minute, "every minute"),
    describeField(hour, "every hour"),
    describeField(day, "every day"),
    describeField(month, "every month"),
    describeField(weekday, "every weekday"),
  ].filter(Boolean);

  return parts.join(", ");
}
