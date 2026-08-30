import { useState } from 'react';
import { useT } from '../i18n';
import { ENV_SOURCE_KEY, envSource, type BootStatus } from '../types';
import { Modal } from './Modal';

interface Props {
  boot: BootStatus;
  /** 重跑一次真的開機偵測(boot_status),回來後 boot prop 會換新 ——
      「重新偵測」按鈕的整個實作,沒有假進度。 */
  onReprobe: () => Promise<void>;
  /** Close and go make the first card — the path the board exists for. */
  onNewTask: () => void;
  /** Close and open a plain session instead — no card, no worktree. */
  onNewSession: () => void;
  onClose: () => void;
}

/**
 * The first-run panel: what this machine already has, then the mental
 * model in three sentences.
 *
 * Everything in the detection list is a probe the app has already run —
 * the login-shell environment, each agent CLI on that PATH, the messaging
 * version gate. Onboarding that asks for what the system already knows
 * reads as broken; onboarding that shows its findings earns trust before
 * the first card exists. Shown once at first run — and reopenable from
 * the palette or the environment panel, probes rerun, flags untouched.
 */
export function WelcomeDialog({ boot, onReprobe, onNewTask, onNewSession, onClose }: Props) {
  const t = useT();
  const agents = boot.agents ?? [];
  /** 偵測進行中:按鈕停用(0.4,系統唯一許可的停用寫法),不轉圈 ——
      偵測快到不值得動畫,慢的話停用狀態本身就是誠實的回答。 */
  const [probing, setProbing] = useState(false);
  /** 一顆 CLI 都沒有:卡片照開,attempt 需要 CLI —— banner 把界線說清楚。 */
  const noAgents = agents.length > 0 && agents.every((a) => a.path === null);

  const probeAgain = () => {
    if (probing) return;
    setProbing(true);
    void onReprobe().finally(() => setProbing(false));
  };

  return (
    <Modal onCancel={onClose}>
      <h2>{t('welcome.title')}</h2>

      <h3 className="modal-section">{t('welcome.found')}</h3>
      <div className="stat">
        <span className="stat-label">{t('env.shell')}</span>
        <span className="stat-value mono">{boot.shell ?? '—'}</span>
      </div>
      <div className="stat">
        <span className="stat-label">{t('env.source')}</span>
        <span className="stat-value mono">{t(ENV_SOURCE_KEY[envSource(boot)])}</span>
      </div>
      {agents.map((a) => (
        <div className="stat" key={a.name} data-testid={`welcome-${a.name}`}>
          <span className="stat-label mono">{a.name}</span>
          {a.path !== null ? (
            <span className="stat-value mono" title={a.path}>
              {/* A CLI whose version gates features says which. The two
                  measured ones carry theirs; the rest are found or not. */}
              {a.version ? `✓ ${a.version}` : '✓'}
            </span>
          ) : (
            <span className="stat-value mono muted">{t('env.cliMissing')}</span>
          )}
        </div>
      ))}
      {noAgents && (
        <div className="no-agent-banner" data-testid="welcome-no-agents">
          <span>{t('welcome.noAgents')}</span>
          <button
            className="chip"
            data-testid="welcome-reprobe"
            disabled={probing}
            onClick={probeAgain}
          >
            {t('welcome.probeAgain')}
          </button>
        </div>
      )}
      <div className="stat">
        <span className="stat-label">{t('env.messaging')}</span>
        <span className="stat-value mono">
          {boot.messaging
            ? '✓'
            : boot.claude
              ? t('env.messagingOld', { version: boot.claudeVersion ?? '—' })
              : t('env.messagingNoClaude')}
        </span>
      </div>

      <h3 className="modal-section">{t('welcome.model')}</h3>
      {/* 三點軌:借看板的 7px 點語彙把心智模型排成三步 —— 空心縫線點
          =一張還沒動的卡、實心 accent 點=跑著的 attempt、紫紅點=合併
          落地。點是裝飾性的(意義在字上),而且全靜態:呼吸與微光留給
          真的在發生的事。 */}
      <div className="welcome-rail">
        <div className="welcome-rail-row">
          <span className="dot rail-card" aria-hidden="true" />
          <span>{t('welcome.model1')}</span>
        </div>
        <div className="welcome-rail-row">
          <span className="dot rail-attempt" aria-hidden="true" />
          <span>{t('welcome.model2')}</span>
        </div>
        <div className="welcome-rail-row">
          <span className="dot rail-finish" aria-hidden="true" />
          <span>{t('welcome.model3')}</span>
        </div>
      </div>

      <div className="modal-actions">
        <button onClick={onClose}>{t('common.close')}</button>
        <button data-testid="welcome-session" onClick={onNewSession}>
          {t('welcome.newSession')}
        </button>
        <button className="primary" data-testid="welcome-card" onClick={onNewTask}>
          {t('welcome.newCard')}
        </button>
      </div>
    </Modal>
  );
}
