// The last line of defence when a render throws: React unmounts the whole tree, top bar and STOP included. This error
// boundary keeps an Emergency stop on screen (the same POST /api/machine/stop as the top bar's STOP, always allowed)
// and offers a reload, so a UI bug never leaves a running laser without a stop button.

import { Component, type ReactNode } from "react";
import { api } from "../api/client.ts";

interface State {
  error: string | null;
  stopped: string | null;
}

export class CrashScreen extends Component<{ children: ReactNode }, State> {
  state: State = { error: null, stopped: null };

  static getDerivedStateFromError(e: unknown): Partial<State> {
    return { error: e instanceof Error ? e.message : String(e) };
  }

  componentDidCatch(e: unknown): void {
    console.error("OpenEtch: the page failed to draw", e);
  }

  private stop = (): void => {
    api.stop().then(
      () => this.setState({ stopped: "Stop sent." }),
      (e: unknown) => this.setState({ stopped: `Stop failed: ${e instanceof Error ? e.message : String(e)}` }),
    );
  };

  render(): ReactNode {
    if (this.state.error === null) return this.props.children;
    return (
      <div className="crash-screen" role="alert">
        <div className="crash-bar">
          <span className="crash-brand">OpenEtch</span>
          <button className="btn btn-stop" onClick={this.stop} title="Emergency stop: abort any run or frame and cancel pending runs. Always allowed.">
            STOP
          </button>
        </div>
        <div className="crash-body">
          <h1>Something went wrong on this page</h1>
          <p>The engine keeps running; STOP above still works. Reload the page to carry on (your document is kept by the engine).</p>
          {this.state.stopped && <p className="crash-stopped">{this.state.stopped}</p>}
          <pre className="crash-error">{this.state.error}</pre>
          <button className="btn btn-primary" onClick={() => location.reload()}>
            Reload
          </button>
        </div>
      </div>
    );
  }
}
