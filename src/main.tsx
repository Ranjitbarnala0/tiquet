import { Component, StrictMode, type ErrorInfo, type ReactNode } from 'react';
import { createRoot } from 'react-dom/client';
import { RotateCcw, TriangleAlert } from 'lucide-react';
import { call } from './api';
import { App } from './App';
import { tr } from './i18n';
import { ToastProvider } from './toast';
import './styles.css';

function report(message: string) {
  call('log.client', { message }).catch(() => {});
}

window.addEventListener('error', (e) => report(`${e.message} (${e.filename}:${e.lineno})`));
window.addEventListener('unhandledrejection', (e) => report(`unhandled rejection: ${e.reason?.message ?? String(e.reason)}`));

// a rendering bug shows a way back instead of a blank window; the ticket in
// progress is kept by the sale screen and comes back after the reload
class Boundary extends Component<{ children: ReactNode }, { failed: boolean }> {
  state = { failed: false };

  static getDerivedStateFromError() {
    return { failed: true };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    report(`${error.message}${info.componentStack ?? ''}`);
  }

  render() {
    if (!this.state.failed) return this.props.children;
    return (
      <div className="fatal">
        <TriangleAlert size={34} />
        <h2>{tr('Algo ha fallado en la pantalla', 'Something went wrong on screen')}</h2>
        <p>{tr('Los tickets guardados no se han perdido. El error queda anotado en el registro.', 'Saved tickets are safe. The error has been written to the log.')}</p>
        <button className="btn btn-primary" onClick={() => location.reload()}>
          <RotateCcw size={16} />
          {tr('Volver a cargar', 'Reload')}
        </button>
      </div>
    );
  }
}

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <Boundary>
      <ToastProvider>
        <App />
      </ToastProvider>
    </Boundary>
  </StrictMode>,
);
