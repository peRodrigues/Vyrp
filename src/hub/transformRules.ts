type Action = "transform" | { transform_prompt: number };
function matches(value: unknown, slot: number | null): boolean {
  return slot === null ? value === "transform" : typeof value === "object" && value !== null && "transform_prompt" in value && value.transform_prompt === slot;
}
export function transformChord(actions: unknown[], slot: number | null): number[] | null {
  const found = actions.find((a): a is [number[], Action] => Array.isArray(a) && Array.isArray(a[0]) && matches(a[1], slot));
  return found?.[0] ?? null;
}
export function setTransformChord(actions: unknown[], slot: number | null, chord: number[] | null): unknown[] {
  const next = actions.filter(a => !(Array.isArray(a) && matches(a[1], slot)));
  if (chord) next.push([chord, slot === null ? "transform" : { transform_prompt: slot }]);
  return next;
}
/** Linear, Unicode-safe diff: common boundaries plus the replaced span. */
export function selectionDiff(before: string, after: string) {
  const a = Array.from(before), b = Array.from(after);
  let start = 0, end = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start++;
  while (end < a.length - start && end < b.length - start && a[a.length - 1 - end] === b[b.length - 1 - end]) end++;
  return { prefix: a.slice(0, start).join(""), removed: a.slice(start, a.length - end).join(""), added: b.slice(start, b.length - end).join(""), suffix: a.slice(a.length - end).join("") };
}
