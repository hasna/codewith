import { Template } from "e2b";

const template = Template()
  .fromDockerfile("codewith-pr-drain.Dockerfile")
  .setWorkdir("/workspace");

const name = process.env.E2B_TEMPLATE_NAME || "codewith-pr-drain";
const cpuCount = Number.parseInt(process.env.E2B_CPU_COUNT || "8", 10);
const memoryMB = Number.parseInt(process.env.E2B_MEMORY_MB || "16384", 10);

if (!process.env.E2B_API_KEY) {
  throw new Error("E2B_API_KEY must be supplied by the caller from a secret ref");
}

const buildInfo = await Template.build(template, name, {
  cpuCount,
  memoryMB,
  skipCache: process.env.E2B_SKIP_CACHE === "1",
  apiKey: process.env.E2B_API_KEY,
});

console.log(JSON.stringify(buildInfo, null, 2));
