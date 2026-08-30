import { useT } from '../i18n';
import { ACTIONS, AGENT_KEYS, GESTURES, KEY_DOCS } from '../actions';
import { Modal } from './Modal';

/**
 * The keyboard, written down — and the pointer beside it. ⌘/Ctrl+/ opens
 * it: the one shortcut worth memorising is the one that lists the rest.
 *
 * Rendered from the action registry, not hand-copied out of it, so a
 * chord can never say one thing here and do another in the palette. The
 * gesture table exists for the same reason in reverse: gestures that live
 * only in tooltips are gestures most people never find.
 */
export function ShortcutsDialog({ onClose }: { onClose: () => void }) {
  const t = useT();
  const chords = [
    ...ACTIONS.filter((a) => a.keys !== null).map((a) => ({
      combo: a.keys as string,
      what: t(a.title),
    })),
    ...KEY_DOCS.map((d) => ({ combo: d.combo, what: t(d.what) })),
  ];

  return (
    <Modal onCancel={onClose}>
      <h2>{t('keys.title')}</h2>
      <table className="keys" data-testid="shortcuts">
        <tbody>
          {chords.map(({ combo, what }) => (
            <tr key={combo + what}>
              <td>
                <kbd>{combo}</kbd>
              </td>
              <td>{what}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <p className="muted small">{t('keys.shellNote')}</p>

      <h3 className="modal-section">{t('keys.agentOwn')}</h3>
      <table className="keys" data-testid="agent-keys">
        <tbody>
          {AGENT_KEYS.map(({ agent, combo, what }) => (
            <tr key={agent + combo}>
              <td>
                <kbd>{combo}</kbd>
              </td>
              <td>
                <span className="chip">{agent}</span> {t(what)}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <p className="muted small">{t('keys.agentOwnNote')}</p>

      <h3 className="modal-section">{t('keys.gestures')}</h3>
      <table className="keys" data-testid="gestures">
        <tbody>
          {GESTURES.map(({ where, what }) => (
            <tr key={where}>
              <td>{t(where)}</td>
              <td>{t(what)}</td>
            </tr>
          ))}
        </tbody>
      </table>

      <div className="modal-actions">
        <button className="primary" onClick={onClose}>
          {t('common.close')}
        </button>
      </div>
    </Modal>
  );
}
