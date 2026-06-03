import path from "node:path";

export function codexPathOverride() {
  return (
    process.env.CODEWITH_EXECUTABLE ??
    process.env.CODEX_EXECUTABLE ??
    path.join(process.cwd(), "..", "..", "codex-rs", "target", "debug", "codewith")
  );
}
