// The JavaScript engine that runs the app, for the report of the decode bench: Hermes
// on the iPhone, another engine in a test.

/** "Hermes" when the app runs on Hermes, else "another engine". */
export function engineName(): string {
  return Reflect.get(globalThis, "HermesInternal") === undefined ? "another engine" : "Hermes";
}
