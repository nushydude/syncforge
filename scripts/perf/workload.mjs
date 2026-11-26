import { execFileSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  utimesSync,
  writeFileSync,
} from "node:fs";
import { cpus, platform, tmpdir, totalmem } from "node:os";
import { join, resolve } from "node:path";
import { performance } from "node:perf_hooks";

const CASES = {
  small: { entries: 10_000, depth: 6, meanFileSize: 256, changedPercent: 10 },
  medium: {
    entries: 100_000,
    depth: 8,
    meanFileSize: 256,
    changedPercent: 10,
  },
};
const DEFAULT_SEED = 20260802;
const MAX_PROGRESS_PER_SECOND = 10;
const MAX_RUN_ITEM_BUFFER = 1_000;

function arg(name, fallback) {
  const index = process.argv.indexOf(`--${name}`);
  return index === -1 ? fallback : (process.argv[index + 1] ?? fallback);
}

function numberArg(name, fallback) {
  const value = Number(arg(name, fallback));
  if (!Number.isInteger(value) || value < 0)
    throw new Error(`--${name} must be a non-negative integer`);
  return value;
}

function rng(seed) {
  let state = seed >>> 0;
  return () => {
    state = (state * 1664525 + 1013904223) >>> 0;
    return state / 2 ** 32;
  };
}

function fixturePath() {
  const path = join(tmpdir(), `syncforge-perf-${process.pid}`);
  if (existsSync(path))
    throw new Error(`Refusing to reuse existing fixture directory: ${path}`);
  mkdirSync(path, { recursive: false });
  return resolve(path);
}

function removeFixture(root) {
  const expected = resolve(join(tmpdir(), `syncforge-perf-${process.pid}`));
  if (resolve(root) !== expected) {
    throw new Error(`Refusing to clean unexpected fixture path: ${root}`);
  }
  rmSync(expected, { recursive: true, force: true });
}

function generateFixture(
  root,
  { entries, depth, meanFileSize, changedPercent },
  seed,
) {
  const random = rng(seed);
  const changedEntries = Math.floor((entries * changedPercent) / 100);
  const left = join(root, "left");
  const right = join(root, "right");
  const changedLeft = join(root, "changed-left");
  const changedRight = join(root, "changed-right");
  for (const side of [left, right, changedLeft, changedRight]) mkdirSync(side);
  for (let i = 0; i < entries; i += 1) {
    const parts = [];
    for (let level = 0; level < depth; level += 1)
      parts.push(`d${String((i + level) % 97).padStart(2, "0")}`);
    const relative = join(...parts, `f${String(i).padStart(7, "0")}.dat`);
    const leftFile = join(left, relative);
    const rightFile = join(right, relative);
    const changedLeftFile = join(changedLeft, relative);
    const changedRightFile = join(changedRight, relative);
    for (const file of [leftFile, rightFile, changedLeftFile, changedRightFile])
      mkdirSync(join(file, ".."), { recursive: true });
    const content = Buffer.alloc(meanFileSize, 0);
    content.write(
      `${seed}:${i}:${Math.floor(random() * 1_000_000)}`.slice(0, meanFileSize),
    );
    writeFileSync(leftFile, content);
    writeFileSync(rightFile, content);
    writeFileSync(changedLeftFile, content);
    const rightContent = Buffer.from(content);
    if (i < changedEntries) rightContent[0] ^= 0xff;
    writeFileSync(changedRightFile, rightContent);
    const fixedTime = new Date(1_700_000_000_000 + i);
    utimesSync(leftFile, fixedTime, fixedTime);
    utimesSync(rightFile, fixedTime, fixedTime);
    utimesSync(changedLeftFile, fixedTime, fixedTime);
    const rightTime =
      i < changedEntries ? new Date(fixedTime.getTime() - 10_000) : fixedTime;
    utimesSync(changedRightFile, rightTime, rightTime);
  }
  return { left, right, changedLeft, changedRight };
}

function walk(root) {
  const files = [];
  const visit = (directory) => {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name);
      if (entry.isDirectory()) visit(path);
      else
        files.push({
          path: path.slice(root.length + 1),
          size: statSync(path).size,
          mtimeMs: statSync(path).mtimeMs,
          content: readFileSync(path).toString("base64"),
        });
    }
  };
  visit(root);
  files.sort((a, b) => a.path.localeCompare(b.path));
  return files;
}

function measure(name, operation, metrics) {
  const start = performance.now();
  const result = operation();
  const elapsedMs = performance.now() - start;
  metrics[name] = { elapsedMs: Number(elapsedMs.toFixed(2)), result };
  return result;
}

function workingSetBytes() {
  if (platform() !== "win32") return process.memoryUsage().rss;
  try {
    const output = execFileSync(
      "powershell",
      [
        "-NoProfile",
        "-Command",
        `(Get-Process -Id ${process.pid}).WorkingSet64`,
      ],
      { encoding: "utf8" },
    );
    return Number(output.trim()) || process.memoryUsage().rss;
  } catch {
    return process.memoryUsage().rss;
  }
}

function runRustScanHarness(root, expectedCopies, expectedBytes) {
  const resultFile = join(root, "rust-counters.json");
  execFileSync(
    "cargo",
    [
      "test",
      "--manifest-path",
      "src-tauri/Cargo.toml",
      "--lib",
      "perf_harness::run_generated_fixture_and_write_counters",
      "--",
      "--nocapture",
    ],
    {
      cwd: resolve("."),
      env: {
        ...process.env,
        PERF_FIXTURE_ROOT: root,
        PERF_RESULT_FILE: resultFile,
        PERF_EXPECTED_COPIES: String(expectedCopies),
        PERF_EXPECTED_BYTES: String(expectedBytes),
      },
      stdio: "inherit",
    },
  );
  return JSON.parse(readFileSync(resultFile, "utf8"));
}

function main() {
  const caseName = arg("case", "small");
  if (!CASES[caseName])
    throw new Error(`Unknown case '${caseName}'. Choose small or medium.`);
  const config = {
    ...CASES[caseName],
    entries: numberArg("entries", CASES[caseName].entries),
    depth: numberArg("depth", CASES[caseName].depth),
    meanFileSize: numberArg("mean-file-size", CASES[caseName].meanFileSize),
    changedPercent: numberArg(
      "changed-percent",
      CASES[caseName].changedPercent,
    ),
  };
  if (config.changedPercent > 100)
    throw new Error("--changed-percent must be between 0 and 100");
  const seed = numberArg("seed", DEFAULT_SEED);
  const changedEntries = Math.floor(
    (config.entries * config.changedPercent) / 100,
  );
  if (changedEntries === 0 || config.meanFileSize === 0)
    throw new Error(
      "workload requires at least one changed file and a nonzero file size",
    );
  const enforceBudgets = process.argv.includes("--enforce-budgets");
  const root = fixturePath();
  const metrics = {};
  let peakWorkingSetBytes = workingSetBytes();
  console.log(`fixture=${root}`);
  try {
    const sides = measure(
      "fixtureGeneration",
      () => generateFixture(root, config, seed),
      metrics,
    );
    const left = measure("proxyScan", () => walk(sides.left), metrics);
    const right = measure("proxyScanRight", () => walk(sides.right), metrics);
    if (left.length !== config.entries || right.length !== config.entries)
      throw new Error("generated no-op fixture has unexpected file count");
    peakWorkingSetBytes = Math.max(peakWorkingSetBytes, workingSetBytes());
    const plan = measure(
      "proxyPlanBuild",
      () =>
        left.map((entry, index) => ({
          ...entry,
          changed: index < changedEntries,
        })),
      metrics,
    );
    measure(
      "proxyNoOpRun",
      () => plan.filter((entry) => !entry.changed).length,
      metrics,
    );
    measure(
      "proxyChangedFileRun",
      () => plan.filter((entry) => entry.changed).length,
      metrics,
    );
    measure("proxyHistoryDetailQuery", () => plan.slice(0, 200), metrics);
    measure(
      "proxyDuplicateScan",
      () => {
        const groups = new Map();
        for (const entry of left)
          groups.set(entry.size, (groups.get(entry.size) ?? 0) + 1);
        return groups.size;
      },
      metrics,
    );
    measure(
      "proxySnifferScan",
      () => left.reduce((total, entry) => total + entry.size, 0),
      metrics,
    );
    const rustCounters = measure(
      "rustRun",
      () =>
        runRustScanHarness(
          root,
          changedEntries,
          changedEntries * config.meanFileSize,
        ),
      metrics,
    );
    peakWorkingSetBytes = Math.max(peakWorkingSetBytes, workingSetBytes());
    const unchangedLeft = walk(sides.left);
    const unchangedRight = walk(sides.right);
    if (
      JSON.stringify(left) !== JSON.stringify(unchangedLeft) ||
      JSON.stringify(right) !== JSON.stringify(unchangedRight)
    )
      throw new Error("no-op run mutated fixture files");
    const copiedLeft = walk(sides.changedLeft);
    const copiedRight = walk(sides.changedRight);
    if (
      copiedLeft.length !== config.entries ||
      copiedRight.length !== config.entries ||
      copiedLeft.some(
        (file, i) =>
          file.path !== copiedRight[i].path ||
          file.size !== copiedRight[i].size ||
          file.content !== copiedRight[i].content,
      )
    )
      throw new Error("changed run output differs from source");
    const progressBudgets = Object.fromEntries(
      Object.entries(rustCounters).map(([phase, counters]) => [
        phase,
        Math.ceil((counters.elapsedMs / 1000) * MAX_PROGRESS_PER_SECOND) + 10,
      ]),
    );
    const report = {
      metadata: {
        case: caseName,
        seed,
        config,
        node: process.version,
        platform: `${platform()} ${process.arch}`,
        cpuCount: cpus().length,
        totalMemoryBytes: totalmem(),
        coldCache: "unknown",
        buildProfile: "working-tree harness",
      },
      fixture: {
        path: root,
        leftEntries: left.length,
        rightEntries: right.length,
      },
      productionRuns: rustCounters,
      budgets: {
        maxProgressEvents: progressBudgets,
        maxProgressEventsPerSecond: MAX_PROGRESS_PER_SECOND,
        phaseAndSchedulerAllowance: 10,
        maxRunItemBuffer: MAX_RUN_ITEM_BUFFER,
        enforced: enforceBudgets,
      },
      sampledNodeParentWorkingSetBytes: peakWorkingSetBytes,
      phases: Object.fromEntries(
        Object.entries(metrics).map(([name, value]) => [
          name,
          {
            elapsedMs: value.elapsedMs,
            resultCount: Array.isArray(value.result)
              ? value.result.length
              : typeof value.result === "number"
                ? value.result
                : undefined,
          },
        ]),
      ),
    };
    console.log(JSON.stringify(report, null, 2));
    for (const [phase, counters] of Object.entries(rustCounters)) {
      if (
        (platform() === "win32" || platform() === "linux") &&
        !counters.rustProcessPeakResidentBytesAtRunEnd
      )
        throw new Error(`${phase} did not report Rust process memory`);
      // Coalescing also permits phase changes and terminal/pending events. Measure only
      // the engine run, with a fixed allowance for these and scheduler jitter.
      const progressBudget = progressBudgets[phase];
      if (enforceBudgets && counters.progressEvents > progressBudget)
        throw new Error(
          `${phase} progress budget exceeded: ${counters.progressEvents} > ${progressBudget}`,
        );
      if (enforceBudgets && counters.maxRunItemBuffer > MAX_RUN_ITEM_BUFFER)
        throw new Error(
          `${phase} run-item buffer budget exceeded: ${counters.maxRunItemBuffer} > ${MAX_RUN_ITEM_BUFFER}`,
        );
    }
  } finally {
    removeFixture(root);
  }
}

try {
  main();
} catch (error) {
  console.error(error instanceof Error ? error.message : error);
  process.exitCode = 1;
}
