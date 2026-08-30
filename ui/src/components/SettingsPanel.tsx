import { useEffect, useState } from 'react';
import type * as React from 'react';
import { LOCALE_NAME, LOCALES, useI18n, type Locale, type MessageKey, type TFn } from '../i18n';
import { api } from '../api';
import { joinArgs, splitArgs } from '../profiles';
import {
  ENV_SOURCE_KEY,
  envSource,
  type BootStatus,
  type NotifyPrefs,
  type UpdateAvailable,
  type UpdateStatus,
} from '../types';
import { setTermSr, termSrEnabled } from '../termSr';
import { Icon } from './Icon';
import { Modal } from './Modal';
import {
  applyTheme,
  contrast,
  currentTheme,
  derive,
  loadStored,
  onColor,
  PRESETS,
  type Primaries,
  type StoredTheme,
} from '../theme';

/** The written-down half of the product, one per interface language. The
 *  READMEs are the documentation — this is the app admitting where they are. */
const DOCS_URL: Record<Locale, string> = {
  en: 'https://github.com/KCL1104/marol#readme',
  'zh-TW': 'https://github.com/KCL1104/marol/blob/main/README.zh-TW.md',
};

type SectionId =
  | 'general'
  | 'sessions'
  | 'terminal'
  | 'notifications'
  | 'agents'
  | 'updates'
  | 'diagnostics'
  | 'advanced';

/**
 * The navigation and the search index in one table.
 *
 * A section is its title plus the name of every setting inside it, so
 * "list the sections" and "find a setting by name" cannot drift apart —
 * the same discipline the action registry holds for verbs. Terms are
 * message keys rather than strings: the search then reads whatever the
 * interface is currently saying, in either language.
 */
const SECTIONS: readonly { id: SectionId; title: MessageKey; terms: readonly MessageKey[] }[] = [
  {
    id: 'general',
    title: 'set.general',
    terms: [
      'env.language',
      'welcome.reopen',
      'coach.replay',
      'env.docs',
      'env.theme',
      'theme.custom',
      'theme.light',
    ],
  },
  {
    id: 'sessions',
    title: 'set.sessions',
    terms: ['set.prompting', 'set.openTemplate', 'env.checkpoints', 'ckpt.onStop'],
  },
  { id: 'terminal', title: 'set.terminal', terms: ['env.termSr', 'termSr.toggle'] },
  {
    id: 'notifications',
    title: 'env.notifications',
    terms: ['notify.permission', 'notify.input', 'notify.done', 'notify.test'],
  },
  { id: 'agents', title: 'env.profiles', terms: ['profile.add', 'profile.save'] },
  {
    id: 'updates',
    title: 'set.updates',
    terms: ['up.check', 'up.enabled', 'up.apply', 'env.version', 'up.lastCheck'],
  },
  {
    id: 'diagnostics',
    title: 'env.diagnostics',
    terms: [
      'env.shell',
      'env.source',
      'env.varCount',
      'env.claude',
      'env.messaging',
      'env.channels',
      'env.db',
      'env.version',
    ],
  },
  { id: 'advanced', title: 'set.advanced', terms: ['set.licenses'] },
];

/**
 * What ships inside the binary that somebody else wrote.
 *
 * Apache-2.0 asks this of us in return for what we took, and a list nobody
 * can find satisfies nobody. Names and licence identifiers are not prose and
 * do not translate, so they live here rather than in the catalogue — a
 * hand-written paragraph about them would rot the first time a dependency
 * changed.
 */
const LICENSES: readonly (readonly [string, string])[] = [
  ['React', 'MIT'],
  ['xterm.js', 'MIT'],
  ['CodeMirror 6', 'MIT'],
  ['IBM Plex Mono', 'SIL OFL 1.1'],
  ['Tauri', 'MIT / Apache-2.0'],
  ['portable-pty (wezterm)', 'MIT'],
  ['rusqlite', 'MIT'],
  ['SQLite', 'public domain'],
  ['serde · tokio · anyhow · uuid · base64 · dirs', 'MIT / Apache-2.0'],
];

/**
 * Everything about how the desk itself is set up, as opposed to any one
 * session: language and theme, the opening prompt, notifications, launch
 * profiles — and diagnostics.
 *
 * It began as the diagnostics panel alone, which is what the old name
 * `EnvPanel` recorded, and the settings grew into it until diagnostics were
 * one section of seven. Worth keeping in view rather than tidying away: a GUI
 * process inherits a stub PATH, so "what environment do the agents actually
 * get" is still the question to bring here when an MCP server or a toolchain
 * behaves differently than it does in Terminal.app.
 */
export function SettingsPanel({
  boot,
  onClose,
  onShowWelcome,
  onReplayCoach,
}: {
  boot: BootStatus;
  onClose: () => void;
  /** 重開歡迎面板(偵測重跑、旗標不動)。App 會先關掉這個面板 ——
      兩層 modal 疊著,Esc 與焦點圈就說不清楚誰的了。 */
  onShowWelcome: () => void;
  /** 忘掉五個 coach 的已讀記號,讓它們各自在下次那一刻重新教一次。 */
  onReplayCoach: () => void;
}) {
  const { t, locale, setLocale } = useI18n();
  /** Unsaved profile edits guard the backdrop, exactly as a typed prompt
   *  does — the panel mixes settings and diagnostics, and losing the one
   *  to a stray click aimed at the other is the mix's worst failure. */
  const [dirty, setDirty] = useState(false);
  const [section, setSection] = useState<SectionId>('general');
  const [query, setQuery] = useState('');

  /**
   * Matching runs over what a person reads, not over keys: the index holds
   * message keys and the search reads their translations, so it works in
   * whichever language the interface happens to be speaking.
   */
  const q = query.trim().toLowerCase();
  const hits: { id: SectionId; title: MessageKey; found: MessageKey[] }[] | null =
    q === ''
      ? null
      : SECTIONS.map((s) => ({
          id: s.id,
          title: s.title,
          found: [s.title, ...s.terms].filter((k) => t(k).toLowerCase().includes(q)),
        })).filter((s) => s.found.length > 0);
  /** One shape for the rail whether or not a search is running, so the
   *  markup below never has to ask which case it is in. */
  const nav = hits ?? SECTIONS.map((s) => ({ id: s.id, title: s.title, found: [] as MessageKey[] }));

  return (
    <Modal onCancel={onClose} dirty={dirty} wide>
        <h2>{t('common.env')}</h2>

        <div className="settings">
          <div className="settings-nav">
            <input
              className="settings-search"
              value={query}
              placeholder={t('set.search')}
              data-testid="settings-search"
              aria-label={t('set.search')}
              onChange={(e) => setQuery(e.target.value)}
            />
            {nav.map((s) => (
              <div key={s.id}>
                <button
                  className={`settings-nav-item${s.id === section ? ' active' : ''}`}
                  data-testid={`sec-${s.id}`}
                  aria-current={s.id === section}
                  onClick={() => setSection(s.id)}
                >
                  {t(s.title)}
                </button>
                {/* A hit is the setting's own name, so the search answers
                    "where does this live" and not merely "somewhere in
                    here". Choosing one takes you there and clears the box. */}
                {s.found
                  .filter((k) => k !== s.title)
                  .map((k) => (
                    <button
                      key={k}
                      className="settings-hit"
                      onClick={() => {
                        setSection(s.id);
                        setQuery('');
                      }}
                    >
                      {t(k)}
                    </button>
                  ))}
              </div>
            ))}
            {hits?.length === 0 && <p className="muted small">{t('palette.empty')}</p>}
          </div>

          <div className="settings-body" data-testid="settings-body">
            {section === 'general' && (
              <>
                <label htmlFor="locale-select">{t('env.language')}</label>
                <select
                  id="locale-select"
                  data-testid="locale-select"
                  value={locale}
                  onChange={(e) => setLocale(e.target.value as Locale)}
                >
                  {LOCALES.map((l) => (
                    <option key={l} value={l}>
                      {LOCALE_NAME[l]}
                    </option>
                  ))}
                </select>

                {/* 歡迎面板本來就是啟動偵測的第一次亮相 —— 這裡給一條回去
                    重看的路,順便重跑偵測。面板是門口,導覽是課,兩者不同,
                    所以是兩顆鈕。

                    說明文件的門也開在這裡:介面上的字被刻意收短了,教學搬進
                    了導覽與 README;沒有這扇門,那不是搬家,是把知識丟掉。
                    連結跟著介面語言走 —— 讀中文的人不該被丟到英文那份。

                    放在「一般」而不是「進階」:這三扇門都是給還在摸索的人開
                    的,而摸索的人最不會去點一個叫「進階」的東西。 */}
                <div className="row welcome-reopen-row">
                  <button data-testid="show-welcome" onClick={onShowWelcome}>
                    {t('welcome.reopen')}
                  </button>
                  <button data-testid="replay-coach" onClick={onReplayCoach}>
                    {t('coach.replay')}
                  </button>
                  <button
                    data-testid="open-docs"
                    onClick={() => void api.openExternal(DOCS_URL[locale])}
                  >
                    {t('env.docs')}
                  </button>
                </div>

                <Theming />
              </>
            )}

            {section === 'sessions' && (
              <>
                <h3 className="modal-section">{t('set.prompting')}</h3>
                <p className="muted small">{t('set.promptingHint')}</p>
                <Stat label="prompt-template.md" value={boot.promptTemplate ?? '—'} />
                {boot.promptTemplate && (
                  <div className="row welcome-reopen-row">
                    <button
                      data-testid="open-template"
                      onClick={() => void api.openPath(boot.promptTemplate as string)}
                    >
                      {t('set.openTemplate')}
                    </button>
                  </div>
                )}
                <Checkpoints />
              </>
            )}

            {section === 'terminal' && (
              <>
                <TermSr />
                <Note testid="note-scrollback">{t('note.scrollback')}</Note>
              </>
            )}

            {section === 'notifications' && <Notifications />}

            {section === 'updates' && <Updates />}

            {section === 'agents' && (
              <>
                <Profiles onDirty={setDirty} />
                <Note testid="note-agents">{t('note.agents')}</Note>
              </>
            )}

            {section === 'diagnostics' && (
              <>
                {/* The doctor half: what the agents actually inherit. */}
                <h3 className="modal-section">{t('env.diagnostics')}</h3>
                {/* First, because it is the first thing anybody reporting a
                    bug is asked for — and until there was an updater, it was
                    the one fact the app knew about itself and never said. */}
                <AppVersion />
                <Stat label={t('env.shell')} value={boot.shell ?? '—'} />
                <Stat label={t('env.source')} value={t(ENV_SOURCE_KEY[envSource(boot)])} />
                <Stat label={t('env.varCount')} value={String(boot.envVarCount ?? 0)} />
                {/* Both CLIs this app knows how to drive, each with the two
                    facts that decide what a card can do: whether it is
                    there, and whether the version that is there reports
                    status. "Installed" and "will tell you what it is doing"
                    are different answers, and only listing the first would
                    stay silent about the commonest reason a card shows no
                    signal. */}
                <Stat
                  label={t('env.claude')}
                  value={cliLine(boot.claude, boot.claudeVersion, reports(boot, 'claude'), t)}
                />
                <Stat
                  label={t('env.codex')}
                  value={cliLine(boot.codex, boot.codexVersion, reports(boot, 'codex'), t)}
                />
                {/* Two facts about Codex a person can only otherwise learn by
                    being confused first. Both are Codex's, not this desk's,
                    and neither is a defect to be fixed here — which is
                    exactly why they belong in writing rather than in a
                    workaround. */}
                {boot.codex && <Note testid="note-codex">{t('note.codexTrust')}</Note>}
                {boot.codex && <Note testid="note-codex-idle">{t('note.codexIdle')}</Note>}
                {/* Whether cards' agents can message each other. The feature
                    is the CLI's own; what this desk adds is naming each
                    session after its card so messages have somewhere to go. */}
                <Stat
                  label={t('env.messaging')}
                  value={
                    boot.messaging
                      ? `✓ · claude ${boot.claudeVersion ?? ''}`.trim()
                      : boot.claude
                        ? t('env.messagingOld', { version: boot.claudeVersion ?? '—' })
                        : // No claude here at all. "Needs Claude Code >= 2.1.224
                          // (found —)" reads as a chore, and it is not one: a
                          // desk that runs codex is not missing anything it
                          // asked for. Name whose feature it is and stop.
                          t('env.messagingNoClaude')
                  }
                />
                {/* One row per world that has actually been reached. The
                    channel declines silently and correctly, which is exactly
                    what makes it worth counting: a distro with no `sh` on the
                    far side, or a pool that is always contended, behaves like
                    a working one and is only slower. Local has no row because
                    it has no doorway to save a crossing of. */}
                {(boot.channels ?? []).map((c) => (
                  <Stat
                    key={c.world}
                    label={`${t('env.channels')} · ${c.world}`}
                    value={
                      t('env.channelsRow', {
                        held: String(c.held),
                        total: String(c.held + c.spawned + c.lost),
                      }) + (c.lost > 0 ? t('env.channelsLost', { n: String(c.lost) }) : '')
                    }
                  />
                ))}
                <Stat label={t('env.db')} value={boot.db ?? '—'} />
                {!boot.envResolved && <p className="muted small">{t('env.degraded')}</p>}
                <label>PATH</label>
                <div className="chips">
                  {splitPath(boot.path ?? '').map((p, i) => (
                    <span className="chip mono" key={`${p}-${i}`}>
                      {p}
                    </span>
                  ))}
                </div>
              </>
            )}

            {section === 'advanced' && (
              <>
                <Note testid="note-telemetry">{t('note.telemetry')}</Note>
                <h3 className="modal-section">{t('set.licenses')}</h3>
                {/* Data, not prose: names and licences do not translate, and
                    a hand-written paragraph about them would rot silently. */}
                <div className="licenses" data-testid="licenses">
                  {LICENSES.map(([who, what]) => (
                    <div className="stat" key={who}>
                      <span className="stat-label">{who}</span>
                      <span className="stat-value mono">{what}</span>
                    </div>
                  ))}
                </div>
              </>
            )}
          </div>
        </div>

        <div className="modal-actions">
          <button className="primary" onClick={onClose}>
            {t('common.close')}
            <kbd>Esc</kbd>
          </button>
        </div>
    </Modal>
  );
}

/**
 * Why the setting you are looking for is not here.
 *
 * Every refusal in this product ships with its full reason — but the reasons
 * live in the README and the decision docs, which is everywhere except the
 * moment somebody opens settings and cannot find the switch. This is that
 * reason, put where the search for it ends. Deliberately not a warning: it
 * is neither a problem nor an error, it is the answer.
 */
function Note({ children, testid }: { children: React.ReactNode; testid: string }) {
  return (
    <p className="settings-note" data-testid={testid}>
      {children}
    </p>
  );
}

/**
 * Which notifications the desk raises. Three toggles, applied on click —
 * a preference is not a form — and a test button, because "is it even
 * working" is otherwise only answerable by waiting for an agent to block.
 * They fire only while the window is elsewhere; in front of the app the
 * interface itself already says everything.
 */
function Notifications() {
  const { t } = useI18n();
  const [prefs, setPrefs] = useState<NotifyPrefs | null>(null);
  const [tested, setTested] = useState(false);

  useEffect(() => {
    void api
      .notifyPrefs()
      .then(setPrefs)
      .catch(() => {
        /* the panel's other sections still work; the row simply stays out */
      });
  }, []);

  if (prefs === null) return null;

  const toggle = (key: keyof NotifyPrefs) => {
    const next = { ...prefs, [key]: !prefs[key] };
    setPrefs(next);
    void api.setNotifyPrefs(next).catch(() => {
      // The next open re-reads what actually stuck.
      setPrefs(prefs);
    });
  };

  const rows: { key: keyof NotifyPrefs; label: string }[] = [
    { key: 'permission', label: t('notify.permission') },
    { key: 'input', label: t('notify.input') },
    { key: 'done', label: t('notify.done') },
  ];

  return (
    <div data-testid="notifications">
      <h3 className="modal-section">{t('env.notifications')}</h3>
      <p className="muted small">{t('notify.hint')}</p>
      {rows.map(({ key, label }) => (
        <label className="notify-row" key={key}>
          <input
            type="checkbox"
            checked={prefs[key]}
            data-testid={`notify-${key}`}
            onChange={() => toggle(key)}
          />
          {label}
        </label>
      ))}
      <div className="row notify-test-row">
        <button
          data-testid="notify-test"
          onClick={() => {
            setTested(true);
            void api.testNotification().catch(() => setTested(false));
          }}
        >
          {tested ? t('notify.sent') : t('notify.test')}
        </button>
      </div>
    </div>
  );
}

/**
 * The running build's own version.
 *
 * Its own row rather than a field on `BootStatus`, because it is answered by
 * a different thing: boot reports what the *environment* has — which shell,
 * which agent CLIs, which database — and this is what *this binary* is. They
 * were never the same question; the app simply had no way to ask the second
 * one until the updater needed it.
 */
function AppVersion() {
  const { t } = useI18n();
  const [version, setVersion] = useState<string | null>(null);

  useEffect(() => {
    void api
      .updateStatus()
      .then((s) => setVersion(s.version))
      .catch(() => {
        /* the rest of the diagnostics still answer */
      });
  }, []);

  if (!version) return null;
  return <Stat label={t('env.version')} value={version} />;
}

/**
 * Updating in place.
 *
 * Three things this section deliberately does *not* do. It does not apply
 * anything on its own: a download starts because somebody pressed a button,
 * the same division this desk keeps everywhere else between a machine-composed
 * thing and the human who sends it. It does not report a failed check — a
 * courtesy that interrupts the work to say it could not be performed has
 * become a cost. And it does not offer a button it cannot honour: a build
 * without a key, or a copy a package manager owns, says so where somebody
 * went looking for the button, which is the same shape as every other
 * refusal in this panel.
 */
function Updates() {
  const { t } = useI18n();
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const [found, setFound] = useState<UpdateAvailable | null>(null);
  const [checking, setChecking] = useState(false);
  const [applying, setApplying] = useState(false);
  const [pct, setPct] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void api
      .updateStatus()
      .then(setStatus)
      .catch(() => {
        /* the rest of the panel still works; this section stays out */
      });
  }, []);

  /** The download's own progress, the one place in this flow where a number
   *  keeps somebody company. `total` is absent on a server that sends no
   *  content-length, and the bar simply does not appear rather than
   *  inventing a denominator — the same rule the token account keeps. */
  useEffect(() => {
    const un = api.onUpdateProgress(({ got, total }) => {
      setPct(total ? Math.min(100, Math.round((got / total) * 100)) : null);
    });
    return () => void un.then((f) => f());
  }, []);

  if (!status) return null;

  const check = () => {
    setChecking(true);
    setError(null);
    void api
      .updateCheck()
      .then(setFound)
      .catch(() => {
        /* offline, rate-limited, GitHub down: none of these are actionable */
      })
      .finally(() => setChecking(false));
  };

  const apply = (acknowledged: boolean) => {
    setApplying(true);
    setError(null);
    // No success path on purpose: the app restarts into the new version, so
    // the only thing that can come back here is a failure.
    void api.updateApply(acknowledged).catch((e: unknown) => {
      setApplying(false);
      setPct(null);
      setError(String(e));
    });
  };

  return (
    <div data-testid="updates">
      <h3 className="modal-section">{t('up.section')}</h3>

      <Stat label={t('env.version')} value={status.version} />
      <Stat
        label={t('up.lastCheck')}
        value={
          status.lastCheck ? new Date(status.lastCheck * 1000).toLocaleString() : t('up.never')
        }
      />

      {/* The two absences, each said where the button would have been. */}
      {!status.configured && <Note testid="up-unconfigured">{t('up.unconfigured')}</Note>}
      {status.configured && !status.selfContained && (
        <Note testid="up-managed">{t('up.managed')}</Note>
      )}

      {(!status.configured || !status.selfContained) && (
        <div className="row welcome-reopen-row">
          <button data-testid="up-releases" onClick={() => void api.openExternal(status.releases)}>
            {t('up.openReleases')}
          </button>
        </div>
      )}

      {status.configured && status.selfContained && (
        <>
          <div className="row welcome-reopen-row">
            <button data-testid="up-check" disabled={checking || applying} onClick={check}>
              {checking ? t('up.checking') : t('up.check')}
            </button>
          </div>

          {found === null && !checking && (
            <p className="muted small" data-testid="up-current">
              {t('up.current', { version: status.version })}
            </p>
          )}

          {found && (
            <div data-testid="up-found">
              <p>{t('up.found', { version: found.version })}</p>
              {found.notes && (
                <details>
                  <summary>{t('up.notes')}</summary>
                  <pre className="small">{found.notes}</pre>
                </details>
              )}

              {/* What the restart costs, in the two kinds it comes in. Held
                  agents are named too: "3 will come back" is the fact that
                  makes the button pressable, and leaving it out would let
                  somebody assume the worse of the two. */}
              {status.held > 0 && (
                <p className="muted small" data-testid="up-held">
                  {t('up.held', { n: String(status.held) })}
                </p>
              )}
              {status.lost > 0 && (
                <p className="small" data-testid="up-lost">
                  ⚠ {t('up.lost', { n: String(status.lost) })}
                </p>
              )}

              <p className="muted small" data-testid="up-backup">
                {t('up.backup', { path: `${status.version}` })}
              </p>

              <div className="row welcome-reopen-row">
                <button
                  data-testid="up-apply"
                  disabled={applying}
                  onClick={() => apply(status.lost > 0)}
                >
                  {applying
                    ? pct === null
                      ? t('up.swapping')
                      : t('up.applying', { pct: String(pct) })
                    : status.lost > 0
                      ? t('up.lostConfirm')
                      : t('up.apply')}
                </button>
              </div>
            </div>
          )}

          {error && <p className="small" data-testid="up-error">{error}</p>}
        </>
      )}

      <label className="notify-row">
        <input
          type="checkbox"
          checked={status.enabled}
          data-testid="up-toggle"
          onChange={() => {
            const next = !status.enabled;
            setStatus({ ...status, enabled: next });
            void api
              .setUpdateEnabled(next)
              .catch(() => setStatus({ ...status, enabled: !next }));
          }}
        />
        {t('up.enabled')}
      </label>
      <p className="muted small">{t('up.enabledHint')}</p>
    </div>
  );
}

/**
 * The one checkpoint setting: whether the end of a turn snapshots the
 * worktree. Default on — the retreat that makes letting an agent run
 * affordable — with the off switch here for repos where the walk costs.
 */
function Checkpoints() {
  const { t } = useI18n();
  const [on, setOn] = useState<boolean | null>(null);

  useEffect(() => {
    void api
      .checkpointsEnabled()
      .then(setOn)
      .catch(() => {
        /* the panel's other sections still work; the row simply stays out */
      });
  }, []);

  if (on === null) return null;

  return (
    <div data-testid="checkpoints">
      <h3 className="modal-section">{t('env.checkpoints')}</h3>
      <p className="muted small">{t('ckpt.hint')}</p>
      <label className="notify-row">
        <input
          type="checkbox"
          checked={on}
          data-testid="ckpt-toggle"
          onChange={() => {
            const next = !on;
            setOn(next);
            void api.setCheckpointsEnabled(next).catch(() => setOn(on));
          }}
        />
        {t('ckpt.onStop')}
      </label>
      {/* Which agents this reaches. The label used to answer that in a
          parenthetical, and answered it wrongly — the snapshot hangs off the
          Stop hook, and Codex fires Stop. Naming both is what stops a Codex
          user reading a working feature as somebody else's. */}
      <p className="muted small">{t('ckpt.onStopHint')}</p>
    </div>
  );
}

/**
 * 終端機的螢幕閱讀器模式：一個開關，點下即生效（偏好不是表單），
 * 廣播給每個活著的終端機。提示文字直說代價 —— 換掉 GPU 繪製，換來
 * 一個朗讀器讀得到的終端機（包含每一則授權提示）。預設關閉。
 */
function TermSr() {
  const { t } = useI18n();
  const [on, setOn] = useState(termSrEnabled);

  return (
    <div data-testid="term-sr">
      <h3 className="modal-section">{t('env.termSr')}</h3>
      <p className="muted small">{t('termSr.hint')}</p>
      <label className="notify-row">
        <input
          type="checkbox"
          checked={on}
          data-testid="term-sr-toggle"
          onChange={() => {
            const next = !on;
            setOn(next);
            setTermSr(next);
          }}
        />
        {t('termSr.toggle')}
      </label>
    </div>
  );
}

/** A row as the editor holds it: args still a string, exactly as typed. */
interface Row {
  name: string;
  agent: string;
  args: string;
}

const AGENTS = ['claude', 'codex', 'gemini', 'aider'];

/**
 * The profile editor: the whole list, edited in place, saved as a whole.
 * There are few enough profiles that per-row saves would only add ways for
 * the screen and the store to disagree. The backend validates the set —
 * empty names, repeats, an agent's own name — and its refusal is shown
 * verbatim, because it names the exact row that cannot be offered.
 */
function Profiles({ onDirty }: { onDirty: (dirty: boolean) => void }) {
  const { t } = useI18n();
  const [rows, setRows] = useState<Row[] | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    void api
      .listProfiles()
      .then((list) =>
        setRows(list.map((p) => ({ name: p.name, agent: p.agent, args: joinArgs(p.args) }))),
      )
      .catch((e) => {
        setRows([]);
        setProblem(String(e));
      });
  }, []);

  const edit = (i: number, patch: Partial<Row>) => {
    setSaved(false);
    onDirty(true);
    setRows((cur) => (cur ? cur.map((r, j) => (j === i ? { ...r, ...patch } : r)) : cur));
  };

  const save = (next: Row[]) => {
    setProblem(null);
    setSaved(false);
    void api
      .saveProfiles(
        next.map((r) => ({ name: r.name.trim(), agent: r.agent, args: splitArgs(r.args) })),
      )
      .then(() => {
        setSaved(true);
        onDirty(false);
      })
      .catch((e) => setProblem(String(e)));
  };

  if (rows === null) return null;

  return (
    <div className="profiles" data-testid="profiles">
      <h3 className="modal-section">{t('env.profiles')}</h3>
      <p className="muted small">{t('env.profilesHint')}</p>

      {rows.map((r, i) => (
        <div className="row profile-row" key={i}>
          <input
            value={r.name}
            placeholder={t('profile.namePlaceholder')}
            data-testid={`profile-name-${i}`}
            onChange={(e) => edit(i, { name: e.target.value })}
          />
          <select
            value={r.agent}
            data-testid={`profile-agent-${i}`}
            onChange={(e) => edit(i, { agent: e.target.value })}
          >
            {AGENTS.map((a) => (
              <option key={a} value={a}>
                {a}
              </option>
            ))}
          </select>
          <input
            className="mono profile-args"
            value={r.args}
            placeholder="--model opus"
            data-testid={`profile-args-${i}`}
            onChange={(e) => edit(i, { args: e.target.value })}
          />
          <button
            className="icon"
            aria-label={t('profile.remove')}
            title={t('profile.remove')}
            onClick={() => {
              // A draft like every other change here: one save contract for
              // the whole list. Instant-persist deletes next to explicit-save
              // edits meant a mis-clicked ✕ committed while a finished edit
              // silently didn't.
              setSaved(false);
              onDirty(true);
              setRows(rows.filter((_, j) => j !== i));
            }}
          >
            ✕
          </button>
        </div>
      ))}

      <div className="row">
        <button
          data-testid="profile-add"
          onClick={() => {
            setSaved(false);
            onDirty(true);
            setRows([...rows, { name: '', agent: 'claude', args: '' }]);
          }}
        >
          {t('profile.add')}
        </button>
        {/* Always offered: with deletion a draft like any other edit, an
            emptied list still needs its save. */}
        <button className="primary" data-testid="profile-save" onClick={() => save(rows)}>
          {saved ? t('profile.saved') : t('profile.save')}
        </button>
      </div>

      {problem && (
        <p className="dialog-error" role="alert" data-testid="profile-error">
          {problem}
        </p>
      )}
    </div>
  );
}

/** The six colors a custom theme asks for, in editor order. */
const COLOR_FIELDS: { key: keyof Primaries; label: string }[] = [
  { key: 'bg', label: 'theme.bg' },
  { key: 'fg', label: 'theme.fg' },
  { key: 'accent', label: 'theme.accent' },
  { key: 'ok', label: 'theme.ok' },
  { key: 'warn', label: 'theme.warn' },
  { key: 'err', label: 'theme.err' },
];

/**
 * Theme choice: presets first, each swatch painted in its own colors so the
 * row is the preview. 自訂 opens the six colors a theme is really made of,
 * with the derived tiers and their contrast shown live — the AA discipline
 * the stylesheet documents, made visible at the moment it is being spent.
 */
function Theming() {
  const { t } = useI18n();
  const [stored, setStored] = useState<StoredTheme>(loadStored);

  const pick = (next: StoredTheme) => {
    setStored(next);
    applyTheme(next);
  };

  const isCustom = stored.preset === 'custom';
  const primaries: Primaries =
    isCustom && 'primaries' in stored
      ? stored.primaries
      : {
          bg: currentTheme().colors.bg,
          fg: currentTheme().colors.fg,
          accent: currentTheme().colors.accent,
          ok: currentTheme().colors.ok,
          warn: currentTheme().colors.warn,
          err: currentTheme().colors.err,
        };
  const light = isCustom && 'light' in stored ? stored.light : currentTheme().light;

  const editColor = (key: keyof Primaries, value: string) =>
    pick({ preset: 'custom', light, primaries: { ...primaries, [key]: value } });

  const derived = derive(primaries);
  const checks: { label: string; ratio: number }[] = [
    { label: t('theme.cText'), ratio: contrast(derived.fg, derived.bg) },
    { label: t('theme.cDim'), ratio: contrast(derived.fgDim, derived.bg2) },
    { label: t('theme.cFaint'), ratio: contrast(derived.fgFaint, derived.bg3) },
    { label: t('theme.cAccent'), ratio: contrast(onColor(derived.accent), derived.accent) },
  ];

  return (
    <div data-testid="theming">
      <h3 className="modal-section">{t('env.theme')}</h3>
      <div className="theme-row">
        {PRESETS.map((p) => (
          <button
            key={p.id}
            className={`theme-swatch${stored.preset === p.id ? ' active' : ''}`}
            data-testid={`theme-${p.id}`}
            aria-pressed={stored.preset === p.id}
            style={{
              background: p.colors.bg,
              color: p.colors.fg,
              borderColor: stored.preset === p.id ? p.colors.accent : p.colors.line,
            }}
            onClick={() => pick({ preset: p.id })}
          >
            <span className="swatch-accent" style={{ background: p.colors.accent }} />
            {t(`theme.${p.id}` as 'theme.ink')}
          </button>
        ))}
        <button
          className={`theme-swatch${isCustom ? ' active' : ''}`}
          data-testid="theme-custom"
          aria-pressed={isCustom}
          onClick={() => pick({ preset: 'custom', light, primaries })}
        >
          <span className="swatch-accent" style={{ background: primaries.accent }} />
          {t('theme.custom')}
        </button>
      </div>

      {isCustom && (
        <div className="theme-editor" data-testid="theme-editor">
          <p className="muted small">{t('theme.customHint')}</p>
          <div className="color-grid">
            {COLOR_FIELDS.map(({ key, label }) => (
              <label className="color-field" key={key}>
                {t(label as 'theme.bg')}
                <input
                  type="color"
                  value={primaries[key]}
                  data-testid={`color-${key}`}
                  onChange={(e) => editColor(key, e.target.value)}
                />
              </label>
            ))}
          </div>
          <label className="theme-light">
            <input
              type="checkbox"
              checked={light}
              onChange={(e) => pick({ preset: 'custom', light: e.target.checked, primaries })}
            />
            {t('theme.light')}
          </label>
          {/* The floor the stylesheet promises, checked while it is spent:
              4.5:1 for every text tier, on the surface it actually sits on. */}
          <div className="contrast-chips" data-testid="contrast-chips">
            {checks.map((c) => (
              <span
                key={c.label}
                className={`contrast-chip ${c.ratio >= 4.5 ? 'pass' : 'fail'}`}
              >
                {c.ratio >= 4.5 ? '✓' : <Icon name="warn" />} {c.label} {c.ratio.toFixed(1)}
              </span>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

/** Windows' PATH separates with `;` and every entry carries a drive-letter
 *  colon, so splitting on `:` would shred `C:\…` into confetti. The drive
 *  pattern guards the one-entry case a `;` check alone would miss. */
function splitPath(path: string): string[] {
  const sep = path.includes(';') || /^[A-Za-z]:[\\/]/.test(path) ? ';' : ':';
  return path.split(sep).filter(Boolean);
}

/** Whether this world's copy of a CLI is one the status hooks apply to.
 *  Absent means the backend did not say, which reads as "no" — the same
 *  direction the launch path takes when a version is unknown. */
function reports(boot: BootStatus, agent: string): boolean {
  return boot.agents?.find((a) => a.name === agent)?.reports === true;
}

/** One CLI's line in the diagnostics: where it is, which version, and
 *  whether that version reports status. A missing CLI says only that —
 *  there is nothing else true about it. */
function cliLine(
  path: string | null | undefined,
  version: string | null | undefined,
  reporting: boolean,
  t: TFn,
): string {
  if (!path) return t('env.cliMissing');
  const parts = [path];
  if (version) parts.push(version);
  parts.push(reporting ? t('env.cliReports') : t('env.cliQuiet'));
  return parts.join(' · ');
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="stat">
      <span className="stat-label">{label}</span>
      <span className="stat-value mono" title={value}>
        {value}
      </span>
    </div>
  );
}
