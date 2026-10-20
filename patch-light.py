from pathlib import Path
p = Path("/tmp/lux-u11/web/src/react.css")
s = p.read_text(encoding="utf-8")
anchor = ".lux-status-pill.is-ok { color: #a5e4bd; background: rgba(96, 194, 132, .13); }"
assert s.count(anchor) == 1, s.count(anchor)
block = (
"\n/* 浅色主题可读性：后台标题/图标按钮写死了 #fff，输入框写死了深色透明底，\n"
"   在白底上会看不见或看不清；这里一律回退到主题变量。 */\n"
"html[data-lux-theme=\"light\"] .lux-admin-panel-heading h2,\n"
"html[data-lux-theme=\"light\"] .lux-admin-page-heading h1,\n"
"html[data-lux-theme=\"light\"] .lux-archive-settings-group > h3,\n"
"html[data-lux-theme=\"light\"] .lux-admin-page h1,\n"
"html[data-lux-theme=\"light\"] .lux-admin-page h2 { color: var(--lux-text); }\n"
"html[data-lux-theme=\"light\"] .lux-icon-button { color: var(--lux-text); }\n"
"html[data-lux-theme=\"light\"] .lux-icon-button:hover { background: rgba(28,28,34,.07); }\n"
"html[data-lux-theme=\"light\"] .lux-admin-settings-form input:not([type=\"checkbox\"]):not([type=\"radio\"]),\n"
"html[data-lux-theme=\"light\"] .lux-admin-settings-form textarea,\n"
"html[data-lux-theme=\"light\"] .lux-admin-settings-form select,\n"
"html[data-lux-theme=\"light\"] .lux-archive-settings-group input:not([type=\"checkbox\"]):not([type=\"radio\"]),\n"
"html[data-lux-theme=\"light\"] .lux-archive-settings-group textarea,\n"
"html[data-lux-theme=\"light\"] .lux-archive-settings-group select {\n"
"  color: var(--lux-text);\n"
"  background: rgba(28,28,34,.04);\n"
"  border-color: var(--lux-line-soft);\n"
"}\n"
"html[data-lux-theme=\"light\"] .lux-archive-settings-status,\n"
"html[data-lux-theme=\"light\"] .lux-archive-settings-fieldset {\n"
"  border-color: var(--lux-line-soft);\n"
"  background: rgba(28,28,34,.035);\n"
"}\n"
)
p.write_text(s.replace(anchor, anchor + block), encoding="utf-8")
print("light theme block appended")