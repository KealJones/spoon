import { loadEnvFile } from "node:process";

export function loadProjectEnvironment(path = ".env"): void {
  try {
    loadEnvFile(path);
  } catch (error) {
    if (hasCode(error, "ENOENT")) return;
    throw error;
  }
}

export function adminTokenFromEnvironment(): string | undefined {
  const token = process.env.EKG_ADMIN_TOKEN?.trim();
  return token || undefined;
}

function hasCode(error: unknown, code: string): boolean {
  return (
    typeof error === "object" &&
    error !== null &&
    "code" in error &&
    error.code === code
  );
}
