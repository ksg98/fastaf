import { Component } from "react";
import type { ErrorInfo, ReactNode } from "react";
import s from "../styles";

interface Props {
  children: ReactNode;
  /** Used to identify the panel in error messages, e.g. "File Browser" */
  label?: string;
  /** Custom fallback UI when an error is caught; uses built-in styles if omitted */
  fallback?: (error: Error, reset: () => void) => ReactNode;
}

interface State {
  error: Error | null;
}

export class ErrorBoundary extends Component<Props, State> {
  constructor(props: Props) {
    super(props);
    this.state = { error: null };
  }

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error(
      `[ErrorBoundary${this.props.label ? ` – ${this.props.label}` : ""}]`,
      error,
      info.componentStack,
    );
  }

  reset = () => {
    this.setState({ error: null });
  };

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;

    if (this.props.fallback) {
      return this.props.fallback(error, this.reset);
    }

    const label = this.props.label ?? "This panel";

    return (
      <div style={s.errorBoundaryWrap}>
        <div style={s.errorBoundaryIcon}>⚠</div>
        <div style={s.errorBoundaryTitle}>{label} failed to render</div>
        <div style={s.errorBoundaryMessage}>{error.message || "Unknown error"}</div>
        <button onClick={this.reset} style={s.errorBoundaryBtn}>
          Retry
        </button>
      </div>
    );
  }
}
