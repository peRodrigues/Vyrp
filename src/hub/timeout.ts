// Chamadas ao backend com prazo: a tela nunca fica presa em "Carregando…" (BUGS-USO).

/** Prazo padrão para as leituras que a tela espera para aparecer. */
export const LOAD_TIMEOUT_MS = 10_000;

/** Erro de prazo esgotado, distinguível de um erro do backend. */
export class TimeoutError extends Error {
  constructor() {
    super("timeout");
    this.name = "TimeoutError";
  }
}

/** Rejeita com {@link TimeoutError} se `promise` não terminar em `ms`. */
export function withTimeout<T>(promise: Promise<T>, ms: number = LOAD_TIMEOUT_MS): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => reject(new TimeoutError()), ms);
    promise.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (err: unknown) => {
        clearTimeout(timer);
        reject(err);
      },
    );
  });
}
