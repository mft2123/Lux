import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Check, Save, Settings2 } from "lucide-react";
import { useEffect, useState } from "react";
import { api } from "../../lib/api/client";
import { queryKeys } from "../../lib/api/query-keys";
import type {
  ArchiveNotifySelection,
  ArchiveSettings,
  ArchiveSettingsPatch,
  ArchiveTelegramSettings,
} from "../../lib/api/types";
import "./archive-settings.css";

// Categories are stored by the label the notify module accepts, so the file
// stays readable and the notify module can keep its English-key aliases.
const NOTIFY_CATEGORIES = [
  { value: "播放", label: "播放" },
  { value: "入库", label: "入库" },
  { value: "整理", label: "整理" },
  { value: "洗版", label: "洗版" },
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
  resource_transfer: "转存",
  drive115_upload: "115妙传",
  magic_upload: "115妙传",
  media_organize_task: "媒体整理",
  checkin: "签到",
  task_complete: "其他任务",
  failure: "失败",
};

const ALL_NOTIFY_CATEGORIES: string[] = NOTIFY_CATEGORIES.map((category) => category.value);

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
  apiId: string;
  apiHash: string;
  phone: string;
  listenMode: string;
  listenChannels: string;
  followList: string;
  notify: string[];
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
  apiId: "",
  apiHash: "",
  phone: "",
  listenMode: "手动",
  listenChannels: "",
  followList: "",
  notify: ALL_NOTIFY_CATEGORIES,
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
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    if (!settings.data) return;
    setDraft(draftFromSettings(settings.data));
    setSaved(false);
  }, [settings.data]);

  const save = useMutation({
    mutationFn: () => {
      const problem = validateDraft(draft);
      if (problem) return Promise.reject(new Error(problem));
      return api.updateArchiveSettings(payloadFromDraft(draft));
    },
    onSuccess: (data) => {
      setDraft(draftFromSettings(data));
      setSaved(true);
      void queryClient.invalidateQueries({ queryKey: queryKeys.adminArchiveSettings });
      void queryClient.invalidateQueries({ queryKey: queryKeys.adminArchiveStatus });
    },
  });

  function update<Key extends keyof ArchiveSettingsDraft>(
    key: Key,
    value: ArchiveSettingsDraft[Key],
  ) {
    setSaved(false);
    setDraft((current) => ({ ...current, [key]: value }));
  }

  function toggleNotify(category: string, checked: boolean) {
    setSaved(false);
    setDraft((current) => {
      const next = checked
        ? Array.from(new Set([...current.notify, category]))
        : current.notify.filter((item) => item !== category);
      return {
        ...current,
        notify: ALL_NOTIFY_CATEGORIES.filter((item) => next.includes(item)),
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
  const errorMessage = save.error instanceof Error ? save.error.message : null;

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
              {NOTIFY_CATEGORIES.map((category) => (
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

        <div className="lux-archive-settings-actions">
          <button
            className="lux-button lux-button-primary lux-settings-save"
            type="button"
            disabled={save.isPending}
            onClick={() => save.mutate()}
          >
            <Save size={16} /> {save.isPending ? "保存中…" : "保存归档设置"}
          </button>
          {saved ? (
            <p className="lux-settings-saved"><Check size={15} /> 已生效，无需重启</p>
          ) : null}
        </div>
        {errorMessage ? <p className="lux-error-copy">{errorMessage}</p> : null}
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
    apiId: settings.telegram.api_id,
    apiHash: "",
    phone: "",
    listenMode: normalizeListenMode(settings.telegram.listen_mode),
    listenChannels: settings.telegram.listen_channels.join("\n"),
    followList: settings.telegram.follow_list.join("\n"),
    notify: selectedNotifyCategories(settings.telegram.notify),
  };
}

function payloadFromDraft(draft: ArchiveSettingsDraft): ArchiveSettingsPatch {
  const telegram: Partial<ArchiveTelegramSettings> = {
    api_id: draft.apiId.trim(),
    listen_mode: draft.listenMode,
    listen_channels: splitLines(draft.listenChannels),
    follow_list: splitLines(draft.followList),
    notify: draft.notify,
    enabled: draft.notifyEnabled,
  };
  // Blank secrets are left out entirely: the server keeps the stored value.
  if (draft.botToken.trim()) telegram.bot_token = draft.botToken.trim();
  if (draft.chatId.trim()) telegram.chat_id = draft.chatId.trim();
  if (draft.apiHash.trim()) telegram.api_hash = draft.apiHash.trim();
  if (draft.phone.trim()) telegram.phone = draft.phone.trim();

  const payload: ArchiveSettingsPatch = {
    watch_dirs: splitLines(draft.watchDirs),
    mirror_root: draft.mirrorRoot.trim(),
    cd2_prefix: draft.cd2Prefix.trim(),
    cd2_mount: draft.cd2Mount.trim(),
    wash_enabled: draft.washEnabled,
    telegram,
  };
  if (draft.tmdbApiKey.trim()) payload.tmdb_api_key = draft.tmdbApiKey.trim();
  const poll = parseNumber(draft.pollSeconds);
  if (poll !== undefined) payload.poll_seconds = Math.trunc(poll);
  const settle = parseNumber(draft.settleSeconds);
  if (settle !== undefined) payload.settle_seconds = Math.trunc(settle);
  const tolerance = parseNumber(draft.washTolerance);
  if (tolerance !== undefined) payload.wash_tolerance_ratio = tolerance;
  return payload;
}

/** Mirror the server-side checks so an obvious typo does not need a round trip. */
function validateDraft(draft: ArchiveSettingsDraft): string | null {
  if (!draft.mirrorRoot.trim()) return "本地镜像库路径不能为空";
  if (!draft.cd2Mount.trim()) return "CD2 挂载点不能为空";
  const poll = parseNumber(draft.pollSeconds);
  if (poll === undefined || poll < 0 || poll > 3600) return "自动轮询间隔必须是 0 到 3600 之间的整数";
  const settle = parseNumber(draft.settleSeconds);
  if (settle === undefined || settle < 0 || settle > 3600) return "稳定等待必须是 0 到 3600 之间的整数";
  const tolerance = parseNumber(draft.washTolerance);
  if (tolerance === undefined || tolerance < 0 || tolerance > 1) {
    return "洗版容差必须是 0 到 1 之间的数字";
  }
  return null;
}

function selectedNotifyCategories(selection: ArchiveNotifySelection): string[] {
  if (selection === null || selection === undefined) return ALL_NOTIFY_CATEGORIES;
  if (Array.isArray(selection)) {
    const items = selection.map((item) => item.trim());
    if (items.some((item) => ["全部", "所有", "all", "*"].includes(item.toLowerCase()))) {
      return ALL_NOTIFY_CATEGORIES;
    }
    const selected = items.map((item) => NOTIFY_ALIASES[item.toLowerCase()] ?? item);
    return ALL_NOTIFY_CATEGORIES.filter((category) => selected.includes(category));
  }
  if (typeof selection === "object") {
    const selected: string[] = [];
    for (const [key, enabled] of Object.entries(selection)) {
      if (!enabled) continue;
      const label = NOTIFY_ALIASES[key.trim().toLowerCase()] ?? key.trim();
      if (ALL_NOTIFY_CATEGORIES.includes(label)) selected.push(label);
    }
    return ALL_NOTIFY_CATEGORIES.filter((category) => selected.includes(category));
  }
  return ALL_NOTIFY_CATEGORIES;
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
