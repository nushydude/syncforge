import type { RunDetail } from "../types";

function escapeCsv(value: string): string {
  if (/[",\n\r]/.test(value)) {
    return `"${value.replace(/"/g, '""')}"`;
  }
  return value;
}

export function runToJson(detail: RunDetail): string {
  return JSON.stringify(detail, null, 2);
}

export function runToCsv(detail: RunDetail): string {
  const { report, items } = detail;
  const lines: string[] = [
    "section,key,value",
    "run,runId," + escapeCsv(report.runId),
    "run,pairId," + escapeCsv(report.pairId),
    "run,startedAt," + report.startedAt,
    "run,finishedAt," + (report.finishedAt ?? ""),
    "run,status," + escapeCsv(report.status),
    "run,filesCopied," + report.filesCopied,
    "run,filesDeleted," + report.filesDeleted,
    "run,bytesTransferred," + report.bytesTransferred,
    "run,errors," + escapeCsv(report.errors.join("; ")),
    "items,path,action,status,message,bytes",
  ];

  for (const item of items) {
    lines.push(
      [
        "item",
        escapeCsv(item.path),
        escapeCsv(item.action),
        escapeCsv(item.status),
        escapeCsv(item.message ?? ""),
        item.bytes ?? "",
      ].join(","),
    );
  }

  return lines.join("\n");
}

export function downloadTextFile(
  filename: string,
  content: string,
  mimeType: string,
): void {
  const blob = new Blob([content], { type: mimeType });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = filename;
  anchor.click();
  URL.revokeObjectURL(url);
}

export function exportRunAsJson(detail: RunDetail): void {
  downloadTextFile(
    `syncforge-run-${detail.report.runId}.json`,
    runToJson(detail),
    "application/json",
  );
}

export function exportRunAsCsv(detail: RunDetail): void {
  downloadTextFile(
    `syncforge-run-${detail.report.runId}.csv`,
    runToCsv(detail),
    "text/csv",
  );
}
