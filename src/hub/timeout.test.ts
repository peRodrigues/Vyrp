import { describe, expect, it, vi } from "vitest";
import { TimeoutError, withTimeout } from "./timeout";

describe("withTimeout", () => {
  it("devolve o valor quando a chamada termina a tempo", async () => {
    await expect(withTimeout(Promise.resolve(7), 50)).resolves.toBe(7);
  });

  it("repassa o erro do backend", async () => {
    await expect(withTimeout(Promise.reject(new Error("falhou")), 50)).rejects.toThrow("falhou");
  });

  it("chamada que nunca responde vira TimeoutError", async () => {
    vi.useFakeTimers();
    const pending = withTimeout(new Promise<number>(() => undefined), 100);
    vi.advanceTimersByTime(100);
    await expect(pending).rejects.toBeInstanceOf(TimeoutError);
    vi.useRealTimers();
  });
});
