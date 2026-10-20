import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Activity, AlertCircle, Check, CheckCircle2, Loader2, Save, Settings2 } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { api } from "../../lib/api/client";
import { queryKeys } from "../../lib/api/query-keys";
import type {
  ArchiveNotifySelection,
  ArchiveSettings,
  ArchiveSettingsPatch,
  ArchiveTelegramSettings,
  ArchiveTransfer115Probe,
  ArchiveTransfer115Settings,
} from "../../lib/api/types";
import "./archive-settings.css";

// The server sends the authoritative list (`known_categories`), so a category
// added in the notify module shows up here without a frontend change. This list
// is only the fallback for an older server that does not send one yet.
const FALLBACK_NOTIFY_CATEGORIES = [
  { value: "播放", label: "播放" },
  { value: "入库", label: "入库" },
  { value: "整理", label: "整理" },
  { value: "洗版", label: "洗版" },
  { value: "重复", label: "重复" },
  { value: "转存", label: "转存" },
  { value: "115妙传", label: "115 妙传" },
  { value: "媒体整理", label: "媒体整理" },
  { value: "签到", label: "签到" },
  { value: "其他任务", label: "其他任务" },
  { value: "失败", label: "失败" },
] as const;

const NOTIFY_ALIASES: Record<string, string> = {
  playback: "播放",
  media_added: "入库",
  organize_complete: "整理",
  organize: "整理",
  archive: "整理",
  wash_result: "洗版",
  duplicate: "重复",
  resource_transfer: "转存",
  drive115_upload: "115妙传",
  magic_upload: "115妙传",
  media_organize_task: "媒体整理",
  checkin: "签到",
  task_complete: "其他任务",
  failure: "失败",
};

const FALLBACK_NOTIFY_VALUES: string[] = FALLBACK_NOTIFY_CATEGORIES.map((category) => category.value);

type NotifyCategoryOption = { value: string; label: string };

function categoryOptions(settings?: ArchiveSettings): NotifyCategoryOption[] {
  const known = settings?.known_categories;
  if (!known || known.length === 0) {
    return FALLBACK_NOTIFY_CATEGORIES.map((category) => ({ ...category }));
  }
  // The backend accepts the Chinese label as well as the English key, and the
  // file stays human readable when it stores labels.
  return known.map((category) => ({ value: category.label, label: category.label }));
}

const LISTEN_MODES = [
  { value: "手动", label: "手动", hint: "只记录频道消息，不自动投递；由管理员手动触发。" },
  { value: "上新", label: "上新", hint: "频道出现新消息时，投递其中的资源链接。" },
  { value: "追更", label: "追更", hint: "仅当链接命中“追更列表”里的剧集名时投递。" },
  { value: "洗版", label: "洗版", hint: "仅当链接体积大于库中同名版本时投递。" },
  { value: "全量", label: "全量", hint: "投递频道里的所有资源链接。" },
] as const;

const LISTEN_MODE_ALIASES: Record<string, string> = {
  manual: "手动",
  new: "上新",
  follow: "追更",
  wash: "洗版",
  all: "全量",
};

type ArchiveSettingsDraft = {
  tmdbApiKey: string;
  watchDirs: string;
  mirrorRoot: string;
  cd2Prefix: string;
  cd2Mount: string;
  pollSeconds: string;
  settleSeconds: string;
  washEnabled: boolean;
  washTolerance: string;
  notifyEnabled: boolean;
  botToken: string;
  chatId: string;
  apiKey: string;
  apiId: string;
  apiHash: string;
  phone: string;
  listenMode: string;
  listenChannels: string;
  followList: string;
  notify: string[];
  /** The value as stored, so an untouched save keeps `null`/object shape. */
  notifyOriginal: ArchiveNotifySelection;
  notifyTouched: boolean;
  transferCookie: string;
  transferTarget: string;
  transferSidecar: string;
  transferTimeout: string;
};

const EMPTY_DRAFT: ArchiveSettingsDraft = {
  tmdbApiKey: "",
  watchDirs: "",
  mirrorRoot: "",
  cd2Prefix: "",
  cd2Mount: "",
  pollSeconds: "0",
  settleSeconds: "5",
  washEnabled: true,
  washTolerance: "0.02",
  notifyEnabled: true,
  botToken: "",
  chatId: "",
  apiKey: "",
  apiId: "",
  apiHash: "",
  phone: "",
  listenMode: "手动",
  listenChannels: "",
  followList: "",
  notify: FALLBACK_NOTIFY_VALUES,
  notifyOriginal: null,
  notifyTouched: false,
  transferCookie: "",
  transferTarget: "转存目录",
  transferSidecar: "",
  transferTimeout: "120",
};

/**
 * The form is split into the same groups the page renders. Each group owns its
 * own save button and sends only its own keys, so a half-finished edit in one
 * group is never committed (or reverted) by saving another.
 */
type SettingsSection = "tmdb" | "paths" | "wash" | "notify" | "listen" | "transfer";

type SectionFeedback = { phase: "idle" | "saving" | "saved" | "error"; message?: string };

type SectionFeedbackMap = Record<SettingsSection, SectionFeedback>;

const EMPTY_FEEDBACK: SectionFeedbackMap = {
  tmdb: { phase: "idle" },
  paths: { phase: "idle" },
  wash: { phase: "idle" },
  notify: { phase: "idle" },
  listen: { phase: "idle" },
  transfer: { phase: "idle" },
};

export function ArchiveSettingsSection() {
  const queryClient = useQueryClient();
  const settings = useQuery({
    queryKey: queryKeys.adminArchiveSettings,
    queryFn: () => api.archiveSettings(),
    refetchOnWindowFocus: false,
  });
  const status = useQuery({
    queryKey: queryKeys.adminArchiveStatus,
    queryFn: () => api.archiveStatus(),
  });
  const [draft, setDraft] = useState<ArchiveSettingsDraft>(EMPTY_DRAFT);
  const [feedback, setFeedback] = useState<SectionFeedbackMap>(EMPTY_FEEDBACK);
  // The form is filled once from the server. A later refetch must not overwrite
  // edits the admin has not saved yet; each save merges its own group back.
  const initialized = useRef(false);
  const categories = categoryOptions(settings.data);

  useEffect(() => {
    if (!settings.data || initialized.current) return;
    setDraft(draftFromSettings(settings.data));
    initialized.current = true;
  }, [settings.data]);

  // Every group saves on its own and sends only its own keys, so saving one
  // group never commits (or reverts) what is still being edited in another.
  const save = useMutation({
    mutationFn: (section: SettingsSection) => {
      const problem = validateSection(section, draft);
      if (problem) throw new Error(problem);
      return api.updateArchiveSettings(payloadForSection(section, draft));
    },
    onMutate: (section) => {
      setFeedback((current) => ({ ...current, [section]: { phase: "saving" } }));
    },
    onSuccess: (data, section) => {
      const merged = draftFromSettings(data);
      setDraft((current) => mergeSection(current, merged, section));
      setFeedback((current) => ({ ...current, [section]: { phase: "saved" } }));
      void queryClient.invalidateQueries({ queryKey: queryKeys.adminArchiveStatus });
      void queryClient.invalidateQueries({ queryKey: queryKeys.adminArchiveSettings });
    },
    onError: (error, section) => {
      setFeedback((current) => ({
        ...current,
        [section]: {
          phase: "error",
          message: error instanceof Error ? error.message : "保存失败",
        },
      }));
    },
  });

  // Probing has to happen on the server: the sidecar sits on the docker network,
  // so only `luxd` can reach it.
  const probe = useMutation({ mutationFn: () => api.probeTransfer115() });

  function update<Key extends keyof ArchiveSettingsDraft>(
    key: Key,
    value: ArchiveSettingsDraft[Key],
  ) {
    // Editing invalidates the "saved" badge of whichever group is now dirty.
    setFeedback((current) => clearSaved(current));
    // A "连接正常" badge must not outlive the edit that changed the address or
    // the cookie it was earned with.
    if (key === "transferCookie" || key === "transferSidecar") probe.reset();
    setDraft((current) => ({ ...current, [key]: value }));
  }

  function toggleNotify(category: string, checked: boolean) {
    setFeedback((current) => clearSaved(current));
    setDraft((current) => {
      const next = checked
        ? Array.from(new Set([...current.notify, category]))
        : current.notify.filter((item) => item !== category);
      return {
        ...current,
        notifyTouched: true,
        notify: categories.map((item) => item.value).filter((item) => next.includes(item)),
      };
    });
  }

  if (!settings.data) {
    return (
      <ArchiveSettingsState
        label={settings.error ? settings.error.message : "正在读取归档设置…"}
        error={Boolean(settings.error)}
      />
    );
  }

  const loaded = settings.data;
  const listenMode =
    LISTEN_MODES.find((mode) => mode.value === draft.listenMode) ?? LISTEN_MODES[0];

  return (
    <section
      className="lux-admin-panel lux-admin-settings-panel"
      aria-labelledby="archive-settings-heading"
    >
      <div className="lux-admin-panel-heading">
        <div>
          <h2 id="archive-settings-heading">归档流水线</h2>
          <p>
            归档、洗版与 Telegram 通知的参数保存在服务器的 archive-settings.json 中，保存后立即生效，无需重启。
          </p>
        </div>
        <Settings2 size={20} className="lux-admin-panel-icon" />
      </div>
      <div className="lux-admin-settings-form">
        <div className="lux-archive-settings-group">
          <h3>TMDB</h3>
          <label>
            <span>TMDB API Key</span>
            <small>用于识别片名、年份、类型与海报。密钥只保存在服务器，读回时打码；留空表示不修改已保存的 Key。</small>
            <input
              type="password"
              autoComplete="off"
              spellCheck={false}
              value={draft.tmdbApiKey}
              placeholder={loaded.tmdb_api_key || "未配置"}
              onChange={(event) => update("tmdbApiKey", event.target.value)}
            />
          </label>
          <SectionSaveBar
            busy={save.isPending && save.variables === "tmdb"}
            feedback={feedback.tmdb}
            onSave={() => save.mutate("tmdb")}
          />
        </div>

        <div className="lux-archive-settings-group">
          <h3>目录与路径</h3>
          <div className="lux-archive-settings-grid">
            <label>
              <span>本地镜像库</span>
              <small>媒体服务器扫描这里：strm + NFO + 海报。必须是 luxd 容器内可访问的绝对路径。</small>
              <input
                type="text"
                autoComplete="off"
                spellCheck={false}
                value={draft.mirrorRoot}
                onChange={(event) => update("mirrorRoot", event.target.value)}
              />
            </label>
            <label>
              <span>CD2 挂载点</span>
              <small>媒体服务器容器里 CloudDrive2 的挂载路径，strm 内容以它为根。</small>
              <input
                type="text"
                autoComplete="off"
                spellCheck={false}
                value={draft.cd2Mount}
                onChange={(event) => update("cd2Mount", event.target.value)}
              />
            </label>
            <label>
              <span>CD2 相对前缀</span>
              <small>归档根在 CD2 挂载内的相对路径，例如“测试库”；留空表示挂载根。</small>
              <input
                type="text"
                autoComplete="off"
                spellCheck={false}
                value={draft.cd2Prefix}
                onChange={(event) => update("cd2Prefix", event.target.value)}
              />
            </label>
          </div>
          <label>
            <span>监听目录</span>
            <small>每行一个目录：新条目会自动搬进“转存目录”，可填多个 CD2 来源。</small>
            <textarea
              rows={3}
              spellCheck={false}
              value={draft.watchDirs}
              placeholder="/testlib/外部上传"
              onChange={(event) => update("watchDirs", event.target.value)}
            />
          </label>
          <SectionSaveBar
            busy={save.isPending && save.variables === "paths"}
            feedback={feedback.paths}
            onSave={() => save.mutate("paths")}
          />
        </div>

        <div className="lux-archive-settings-group">
          <h3>洗版参数</h3>
          <div className="lux-archive-settings-grid">
            <label>
              <span>自动轮询间隔</span>
              <small>0 表示关闭自动归档，只保留手动触发。</small>
              <div className="lux-admin-input-with-suffix">
                <input
                  type="number"
                  min="0"
                  max="3600"
                  value={draft.pollSeconds}
                  onChange={(event) => update("pollSeconds", event.target.value)}
                />
                <em>秒</em>
              </div>
            </label>
            <label>
              <span>稳定等待</span>
              <small>文件在这段时间内没有变化才会被认领，避免搬走正在写入的包。</small>
              <div className="lux-admin-input-with-suffix">
                <input
                  type="number"
                  min="0"
                  max="3600"
                  value={draft.settleSeconds}
                  onChange={(event) => update("settleSeconds", event.target.value)}
                />
                <em>秒</em>
              </div>
            </label>
            <label>
              <span>洗版容差</span>
              <small>新旧包体积差在这个比例内视为同一版本；只有明显更大才洗版。</small>
              <input
                type="number"
                min="0"
                max="1"
                step="0.01"
                value={draft.washTolerance}
                onChange={(event) => update("washTolerance", event.target.value)}
              />
            </label>
          </div>
          <label className="lux-admin-toggle">
            <input
              type="checkbox"
              checked={draft.washEnabled}
              onChange={(event) => update("washEnabled", event.target.checked)}
            />
            <span>启用洗版（新包明显更大时替换旧包）</span>
          </label>
          <SectionSaveBar
            busy={save.isPending && save.variables === "wash"}
            feedback={feedback.wash}
            onSave={() => save.mutate("wash")}
          />
        </div>

        <div className="lux-archive-settings-group">
          <h3>Telegram 通知</h3>
          <div className="lux-archive-settings-grid">
            <label>
              <span>Bot Token</span>
              <small>通知机器人凭证。已保存的值读回时打码，留空表示不修改。</small>
              <input
                type="password"
                autoComplete="off"
                spellCheck={false}
                value={draft.botToken}
                placeholder={loaded.telegram.bot_token || "未配置"}
                onChange={(event) => update("botToken", event.target.value)}
              />
            </label>
            <label>
              <span>Chat ID</span>
              <small>接收通知的会话 id 或 @频道名。同样打码返回，留空表示不修改。</small>
              <input
                type="password"
                autoComplete="off"
                spellCheck={false}
                value={draft.chatId}
                placeholder={loaded.telegram.chat_id || "未配置"}
                onChange={(event) => update("chatId", event.target.value)}
              />
            </label>
            <label>
              <span>Lux 管理 API Key</span>
              <small>旁挂调用 Lux 管理接口用的密钥。打码返回，留空表示不修改。</small>
              <input
                type="password"
                autoComplete="off"
                spellCheck={false}
                value={draft.apiKey}
                placeholder={loaded.telegram.api_key || "未配置"}
                onChange={(event) => update("apiKey", event.target.value)}
              />
            </label>
          </div>
          <label className="lux-admin-toggle">
            <input
              type="checkbox"
              checked={draft.notifyEnabled}
              onChange={(event) => update("notifyEnabled", event.target.checked)}
            />
            <span>启用通知</span>
          </label>
          <div className="lux-archive-settings-fieldset">
            <span className="lux-archive-settings-legend">通知项</span>
            <div className="lux-archive-settings-toggles">
              {categories.map((category) => (
                <label key={category.value} className="lux-admin-toggle">
                  <input
                    type="checkbox"
                    checked={draft.notify.includes(category.value)}
                    onChange={(event) => toggleNotify(category.value, event.target.checked)}
                  />
                  <span>{category.label}</span>
                </label>
              ))}
            </div>
          </div>
          <SectionSaveBar
            busy={save.isPending && save.variables === "notify"}
            feedback={feedback.notify}
            onSave={() => save.mutate("notify")}
          />
        </div>

        <div className="lux-archive-settings-group">
          <h3>115 转存</h3>
          <p>
            链接队列里的 115 分享链接由 transfer115 侧车用这里的 cookie 登录后转存到目标目录；
            侧车读取同一份设置文件。留空表示不修改已保存的值。
          </p>
          <div className="lux-archive-settings-grid">
            <label>
              <span>115 Cookie</span>
              <small>浏览器登录 115 后复制的整串 cookie（UID/CID/SEID/KID）。打码返回，留空表示不修改。</small>
              <input
                type="password"
                autoComplete="off"
                spellCheck={false}
                value={draft.transferCookie}
                placeholder={loaded.transfer115?.cookie || "未配置"}
                onChange={(event) => update("transferCookie", event.target.value)}
              />
            </label>
            <label>
              <span>转存目标目录</span>
              <small>相对归档根的目录，侧车会解析成 115 的目录；不存在时自动创建。</small>
              <input
                type="text"
                autoComplete="off"
                spellCheck={false}
                value={draft.transferTarget}
                placeholder="转存目录"
                onChange={(event) => update("transferTarget", event.target.value)}
              />
            </label>
            <label>
              <span>侧车地址</span>
              <small>例如 http://transfer115:8098；留空表示关闭转存阶段（作业只记 needs_115_client）。</small>
              <input
                type="text"
                autoComplete="off"
                spellCheck={false}
                value={draft.transferSidecar}
                placeholder="http://transfer115:8098"
                onChange={(event) => update("transferSidecar", event.target.value)}
              />
            </label>
            <label>
              <span>转存超时（秒）</span>
              <small>0–600；到点即中断，作业留在队列下一轮重试。</small>
              <input
                type="number"
                min={0}
                max={600}
                value={draft.transferTimeout}
                onChange={(event) => update("transferTimeout", event.target.value)}
              />
            </label>
          </div>
          <SectionSaveBar
            busy={save.isPending && save.variables === "transfer"}
            feedback={feedback.transfer}
            onSave={() => save.mutate("transfer")}
            extra={
              <>
                <button
                  className="lux-button lux-button-secondary"
                  type="button"
                  disabled={probe.isPending || save.isPending}
                  onClick={() => probe.mutate()}
                >
                  {probe.isPending ? (
                    <Loader2 size={16} className="lux-spin" />
                  ) : (
                    <Activity size={16} />
                  )}
                  {probe.isPending ? "测试中…" : "测试连接"}
                </button>
                {probe.isPending ? (
                  <span className="lux-settings-probe is-pending" role="status">
                    <Loader2 size={15} className="lux-spin" /> 正在连接侧车并校验 115 登录…
                  </span>
                ) : null}
                {!probe.isPending && probe.data ? (
                  <span
                    className={
                      probe.data.status === "logged_in"
                        ? "lux-settings-probe is-ok"
                        : "lux-settings-probe is-bad"
                    }
                    role="status"
                    aria-live="polite"
                  >
                    {probe.data.status === "logged_in" ? (
                      <CheckCircle2 size={15} />
                    ) : (
                      <AlertCircle size={15} />
                    )}
                    {describeProbe(probe.data)}
                  </span>
                ) : null}
                {!probe.isPending && probe.error ? (
                  <span className="lux-settings-probe is-bad" role="status" aria-live="polite">
                    <AlertCircle size={15} />
                    {probe.error instanceof Error ? probe.error.message : "测试连接失败"}
                  </span>
                ) : null}
              </>
            }
          />
        </div>

        <div className="lux-archive-settings-group">
          <h3>Telegram 频道监听（Userbot）</h3>
          <p>旁挂的 Telethon 服务读取同一份设置；登录 session 由旁挂服务保存。</p>
          <div className="lux-archive-settings-grid">
            <label>
              <span>API ID</span>
              <small>在 my.telegram.org 创建的应用 id。</small>
              <input
                type="text"
                inputMode="numeric"
                autoComplete="off"
                spellCheck={false}
                value={draft.apiId}
                onChange={(event) => update("apiId", event.target.value)}
              />
            </label>
            <label>
              <span>API Hash</span>
              <small>应用 hash，读回时打码，留空表示不修改。</small>
              <input
                type="password"
                autoComplete="off"
                spellCheck={false}
                value={draft.apiHash}
                placeholder={loaded.telegram.api_hash || "未配置"}
                onChange={(event) => update("apiHash", event.target.value)}
              />
            </label>
            <label>
              <span>手机号</span>
              <small>Userbot 登录用的号码，读回时打码，留空表示不修改。</small>
              <input
                type="password"
                autoComplete="off"
                spellCheck={false}
                value={draft.phone}
                placeholder={loaded.telegram.phone || "未配置"}
                onChange={(event) => update("phone", event.target.value)}
              />
            </label>
          </div>
          <label>
            <span>监听模式</span>
            <small>{listenMode.hint}</small>
            <select
              value={draft.listenMode}
              onChange={(event) => update("listenMode", event.target.value)}
            >
              {LISTEN_MODES.map((mode) => (
                <option key={mode.value} value={mode.value}>{mode.label}</option>
              ))}
            </select>
          </label>
          <div className="lux-archive-settings-grid">
            <label>
              <span>监听频道</span>
              <small>每行一个频道 @用户名 或数字 id。</small>
              <textarea
                rows={3}
                spellCheck={false}
                value={draft.listenChannels}
                placeholder="@channel"
                onChange={(event) => update("listenChannels", event.target.value)}
              />
            </label>
            <label>
              <span>追更列表</span>
              <small>每行一个剧集名；“追更”模式下只有命中这些名字的链接会被投递。</small>
              <textarea
                rows={3}
                spellCheck={false}
                value={draft.followList}
                placeholder="繁花"
                onChange={(event) => update("followList", event.target.value)}
              />
            </label>
          </div>
          <SectionSaveBar
            busy={save.isPending && save.variables === "listen"}
            feedback={feedback.listen}
            onSave={() => save.mutate("listen")}
          />
        </div>

        <div className="lux-archive-settings-group">
          <h3>当前生效状态</h3>
          {status.data ? (
            <div className="lux-archive-settings-status" role="status" aria-live="polite">
              <div><span>归档根目录</span><strong>{status.data.root}</strong></div>
              <div>
                <span>TMDB</span>
                <strong>{status.data.tmdb_configured ? "已配置" : "未配置"}</strong>
              </div>
              <div>
                <span>洗版</span>
                <strong>
                  {status.data.wash_enabled
                    ? `开启（容差 ${status.data.wash_tolerance_ratio}）`
                    : "关闭"}
                </strong>
              </div>
              <div><span>稳定等待</span><strong>{status.data.settle_seconds} 秒</strong></div>
              <div>
                <span>监听目录</span>
                <strong>
                  {status.data.watch_dirs.length > 0 ? status.data.watch_dirs.join("，") : "未设置"}
                </strong>
              </div>
            </div>
          ) : (
            <p className="lux-archive-settings-hint">
              {status.error ? status.error.message : "正在读取归档状态…"}
            </p>
          )}
        </div>
      </div>
    </section>
  );
}

function ArchiveSettingsState({ label, error = false }: { label: string; error?: boolean }) {
  return (
    <section className="lux-admin-page-state" role={error ? "alert" : "status"}>
      <h1>{error ? "归档设置加载失败" : "正在加载归档设置"}</h1>
      <p>{label}</p>
    </section>
  );
}

function draftFromSettings(settings: ArchiveSettings): ArchiveSettingsDraft {
  const categories = categoryOptions(settings);
  return {
    tmdbApiKey: "",
    watchDirs: settings.watch_dirs.join("\n"),
    mirrorRoot: settings.mirror_root,
    cd2Prefix: settings.cd2_prefix,
    cd2Mount: settings.cd2_mount,
    pollSeconds: String(settings.poll_seconds),
    settleSeconds: String(settings.settle_seconds),
    washEnabled: settings.wash_enabled,
    washTolerance: String(settings.wash_tolerance_ratio),
    notifyEnabled: settings.telegram.enabled ?? true,
    botToken: "",
    chatId: "",
    apiKey: "",
    apiId: settings.telegram.api_id,
    apiHash: "",
    phone: "",
    listenMode: normalizeListenMode(settings.telegram.listen_mode),
    listenChannels: settings.telegram.listen_channels.join("\n"),
    followList: settings.telegram.follow_list.join("\n"),
    notify: selectedNotifyCategories(settings.telegram.notify, categories),
    notifyOriginal: settings.telegram.notify,
    notifyTouched: false,
    transferCookie: "",
    transferTarget: settings.transfer115?.target_dir || "转存目录",
    transferSidecar: settings.transfer115?.sidecar_base || "",
    transferTimeout: String(settings.transfer115?.timeout_seconds ?? 120),
  };
}

const SECTION_FIELDS: Record<SettingsSection, (keyof ArchiveSettingsDraft)[]> = {
  tmdb: ["tmdbApiKey"],
  paths: ["watchDirs", "mirrorRoot", "cd2Prefix", "cd2Mount"],
  wash: ["pollSeconds", "settleSeconds", "washEnabled", "washTolerance"],
  notify: ["notifyEnabled", "botToken", "chatId", "apiKey", "notify", "notifyOriginal", "notifyTouched"],
  listen: ["apiId", "apiHash", "phone", "listenMode", "listenChannels", "followList"],
  transfer: ["transferCookie", "transferTarget", "transferSidecar", "transferTimeout"],
};

/** Take back only the fields a section saved, leaving other drafts untouched. */
function mergeSection(
  current: ArchiveSettingsDraft,
  saved: ArchiveSettingsDraft,
  section: SettingsSection,
): ArchiveSettingsDraft {
  const patch = Object.fromEntries(
    SECTION_FIELDS[section].map((field) => [field, saved[field]]),
  ) as Partial<ArchiveSettingsDraft>;
  return { ...current, ...patch };
}

/** Drop the "已生效" badge once the admin starts editing again. */
function clearSaved(current: SectionFeedbackMap): SectionFeedbackMap {
  let changed = false;
  const next = { ...current };
  for (const section of Object.keys(next) as SettingsSection[]) {
    if (next[section].phase === "saved") {
      next[section] = { phase: "idle" };
      changed = true;
    }
  }
  return changed ? next : current;
}

function payloadForSection(
  section: SettingsSection,
  draft: ArchiveSettingsDraft,
): ArchiveSettingsPatch {
  switch (section) {
    case "tmdb": {
      const patch: ArchiveSettingsPatch = {};
      if (draft.tmdbApiKey.trim()) patch.tmdb_api_key = draft.tmdbApiKey.trim();
      return patch;
    }
    case "paths":
      return {
        watch_dirs: splitLines(draft.watchDirs),
        mirror_root: draft.mirrorRoot.trim(),
        cd2_prefix: draft.cd2Prefix.trim(),
        cd2_mount: draft.cd2Mount.trim(),
      };
    case "wash": {
      const patch: ArchiveSettingsPatch = { wash_enabled: draft.washEnabled };
      const poll = parseNumber(draft.pollSeconds);
      if (poll !== undefined) patch.poll_seconds = Math.trunc(poll);
      const settle = parseNumber(draft.settleSeconds);
      if (settle !== undefined) patch.settle_seconds = Math.trunc(settle);
      const tolerance = parseNumber(draft.washTolerance);
      if (tolerance !== undefined) patch.wash_tolerance_ratio = tolerance;
      return patch;
    }
    case "notify": {
      const telegram: Partial<ArchiveTelegramSettings> = {
        enabled: draft.notifyEnabled,
        // Only rewrite the selection when the admin actually toggled something:
        // an untouched save must keep `null` (everything on) or the stored object
        // shape instead of turning it into a fixed list.
        notify: draft.notifyTouched ? draft.notify : draft.notifyOriginal,
      };
      // Blank secrets are left out entirely: the server keeps the stored value.
      if (draft.botToken.trim()) telegram.bot_token = draft.botToken.trim();
      if (draft.chatId.trim()) telegram.chat_id = draft.chatId.trim();
      if (draft.apiKey.trim()) telegram.api_key = draft.apiKey.trim();
      return { telegram };
    }
    case "listen": {
      const telegram: Partial<ArchiveTelegramSettings> = {
        api_id: draft.apiId.trim(),
        listen_mode: draft.listenMode,
        listen_channels: splitLines(draft.listenChannels),
        follow_list: splitLines(draft.followList),
      };
      if (draft.apiHash.trim()) telegram.api_hash = draft.apiHash.trim();
      if (draft.phone.trim()) telegram.phone = draft.phone.trim();
      return { telegram };
    }
    case "transfer": {
      const transfer115: Partial<ArchiveTransfer115Settings> = {
        target_dir: draft.transferTarget.trim(),
        sidecar_base: draft.transferSidecar.trim(),
      };
      const timeout = parseNumber(draft.transferTimeout);
      if (timeout !== undefined) transfer115.timeout_seconds = Math.trunc(timeout);
      // Same rule as the other secrets: blank means "keep the stored cookie".
      if (draft.transferCookie.trim()) transfer115.cookie = draft.transferCookie.trim();
      return { transfer115 };
    }
  }
}

/** Mirror the server-side checks for just the group being saved. */
function validateSection(section: SettingsSection, draft: ArchiveSettingsDraft): string | null {
  if (section === "paths") {
    if (!draft.mirrorRoot.trim()) return "本地镜像库路径不能为空";
    if (!draft.cd2Mount.trim()) return "CD2 挂载点不能为空";
  }
  if (section === "wash") {
    const poll = parseNumber(draft.pollSeconds);
    if (poll === undefined || poll < 0 || poll > 3600) {
      return "自动轮询间隔必须是 0 到 3600 之间的整数";
    }
    const settle = parseNumber(draft.settleSeconds);
    if (settle === undefined || settle < 0 || settle > 3600) {
      return "稳定等待必须是 0 到 3600 之间的整数";
    }
    const tolerance = parseNumber(draft.washTolerance);
    if (tolerance === undefined || tolerance < 0 || tolerance > 1) {
      return "洗版容差必须是 0 到 1 之间的数字";
    }
  }
  if (section === "transfer") {
    const timeout = parseNumber(draft.transferTimeout);
    if (timeout !== undefined && (timeout < 0 || timeout > 600)) {
      return "转存超时必须是 0 到 600 之间的秒数";
    }
  }
  return null;
}

function describeProbe(result: ArchiveTransfer115Probe): string {
  switch (result.status) {
    case "logged_in":
      return "连接正常：侧车在线，115 cookie 有效。";
    case "auth_required":
      return "侧车在线，但 115 cookie 已失效，请重新填写后保存。";
    case "unconfigured":
      return "未配置侧车地址，转存阶段当前是关闭的。";
    case "unreachable":
      return "连不上侧车，检查地址是否正确、容器是否在运行。";
    default:
      return `侧车返回：${result.status}${result.detail ? `（${result.detail}）` : ""}`;
  }
}

function SectionSaveBar({
  busy,
  feedback,
  onSave,
  extra,
}: {
  busy: boolean;
  feedback: SectionFeedback;
  onSave: () => void;
  extra?: ReactNode;
}) {
  return (
    <div className="lux-archive-settings-footer">
      <button
        className="lux-button lux-button-primary"
        type="button"
        disabled={busy}
        onClick={onSave}
      >
        {busy ? <Loader2 size={16} className="lux-spin" /> : <Save size={16} />}
        {busy ? "保存中…" : "保存"}
      </button>
      {extra}
      {feedback.phase === "saved" ? (
        <p className="lux-settings-saved">
          <Check size={15} /> 已生效
        </p>
      ) : null}
      {feedback.phase === "error" && feedback.message ? (
        <p className="lux-error-copy">{feedback.message}</p>
      ) : null}
    </div>
  );
}

function selectedNotifyCategories(
  selection: ArchiveNotifySelection,
  categories: NotifyCategoryOption[],
): string[] {
  const all = categories.map((category) => category.value);
  if (selection === null || selection === undefined) return all;
  if (Array.isArray(selection)) {
    const items = selection.map((item) => item.trim());
    if (items.some((item) => ["全部", "所有", "all", "*"].includes(item.toLowerCase()))) {
      return all;
    }
    const selected = items.map((item) => NOTIFY_ALIASES[item.toLowerCase()] ?? item);
    return all.filter((category) => selected.includes(category));
  }
  if (typeof selection === "object") {
    const selected: string[] = [];
    for (const [key, enabled] of Object.entries(selection)) {
      if (!enabled) continue;
      const label = NOTIFY_ALIASES[key.trim().toLowerCase()] ?? key.trim();
      if (all.includes(label)) selected.push(label);
    }
    return all.filter((category) => selected.includes(category));
  }
  return all;
}

function normalizeListenMode(value: string): string {
  const trimmed = value.trim();
  return LISTEN_MODE_ALIASES[trimmed.toLowerCase()]
    ?? (LISTEN_MODES.some((mode) => mode.value === trimmed) ? trimmed : "手动");
}

function splitLines(value: string): string[] {
  return value
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}

function parseNumber(value: string): number | undefined {
  const trimmed = value.trim();
  if (!trimmed) return undefined;
  const parsed = Number(trimmed);
  return Number.isFinite(parsed) ? parsed : undefined;
}
