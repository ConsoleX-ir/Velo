/**
 * Minimal class-name joiner, in the spirit of shadcn/ui's `cn` (MIT licensed)
 * but without any runtime dependency: falsy inputs are skipped and the
 * remaining strings are joined with single spaces.
 */
export function cn(...inputs: Array<string | false | null | undefined>): string {
  return inputs.filter(Boolean).join(" ");
}

export function errorText(error: unknown): string {
  if (typeof error === "object" && error !== null && "message" in error) return String(error.message);
  return typeof error === "string" ? error : "Something went wrong. Please try again.";
}
