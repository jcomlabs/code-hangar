import { Component, type ErrorInfo, type ReactNode } from "react";

interface Props {
  children: ReactNode;
}

interface State {
  error: Error | null;
}

// Top-level safety net. Without this, any uncaught render error unmounts the
// whole React tree and leaves the user staring at a blank window (a perceived
// "crash"). Here we catch it, keep the app process alive, and offer a clean
// reload — which re-reads the local inventory from the backend. No files on
// disk are ever touched by a render error.
export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    // Log for devtools without taking the app down.
    console.error("Code Hangar caught a render error:", error, info.componentStack);
  }

  private handleReload = () => {
    this.setState({ error: null });
    window.location.reload();
  };

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <div className="app-crash" role="alert">
        <div className="app-crash-card">
          <strong className="app-crash-title">Something went wrong in the interface</strong>
          <p className="app-crash-copy">
            Code Hangar hit an unexpected display error. Your local data, scan roots and files on disk were not
            changed. Reloading reopens the inventory cleanly.
          </p>
          <pre className="app-crash-detail">{error.message || String(error)}</pre>
          <div className="app-crash-actions">
            <button type="button" className="action-button" onClick={this.handleReload}>
              Reload Code Hangar
            </button>
          </div>
          <small className="app-crash-foot">If this keeps happening, fully restart the app. No files on disk are affected.</small>
        </div>
      </div>
    );
  }
}
