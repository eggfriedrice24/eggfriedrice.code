// A probe of the render counter (render-counts.ts): a React Profiler around a part of a
// screen. React calls the Profiler only in a dev build (and a profiling build), so the
// probe counts nothing in a preview build and costs nothing there.

import { Profiler, type ProfilerOnRenderCallback, type ReactNode } from "react";

import { renderCounts, type RenderCounts } from "./render-counts";

export interface RenderProbeProps {
  /** The name of the part, such as `conversation`. */
  readonly id: string;
  /** The counter; the counter of the app by default. */
  readonly counts?: RenderCounts;
  readonly children: ReactNode;
}

/** Counts the commits of `children` as part `id`. */
export function RenderProbe(props: RenderProbeProps) {
  const counts = props.counts ?? renderCounts;
  const onRender: ProfilerOnRenderCallback = (id, phase, actualDuration, _base, _start, commit) => {
    counts.record(id, phase, actualDuration, commit);
  };
  return (
    <Profiler id={props.id} onRender={onRender}>
      {props.children}
    </Profiler>
  );
}
