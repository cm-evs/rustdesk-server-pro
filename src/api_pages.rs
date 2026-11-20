// Web pages for the API service: client OIDC login page and the web console.
// Plain server-rendered HTML shells + vanilla JS calling the JSON APIs in api.rs.
//
// i18n: the console is Chinese by default with an English switch. Static
// text stays Chinese inline in the HTML (the page stays usable without JS,
// and existing page-marker checks keep passing); elements carry
// data-i18n="key" (text), data-i18n-ph="key" (placeholder),
// data-i18n-title="key" (title) or data-i18n-aria="key" (aria-label) and
// the I18N_JS dictionary swaps them at runtime. JS-rendered text goes
// through t('key'). The choice is kept in localStorage ('rdp-lang',
// 'zh' default). API error strings are NOT translated: clients depend
// on their exact text.

pub const CSS: &str = r#"
*{box-sizing:border-box}body{margin:0;font-family:-apple-system,'Segoe UI',Roboto,'Helvetica Neue',Arial,sans-serif;background:#0f1419;color:#e6edf3}
a{color:#58a6ff}.wrap{max-width:1080px;margin:0 auto;padding:24px 16px}
.card{background:#161b22;border:1px solid #30363d;border-radius:10px;padding:20px;margin-bottom:16px}
h1{font-size:20px;margin:0 0 16px}h2{font-size:16px;margin:0 0 12px}
input,select{width:100%;padding:9px 10px;margin:6px 0 12px;background:#0d1117;border:1px solid #30363d;border-radius:6px;color:#e6edf3}
button{background:#1f6feb;color:#fff;border:0;border-radius:6px;padding:9px 16px;cursor:pointer;font-size:14px;line-height:20px;white-space:nowrap;flex-shrink:0}
button:hover{background:#2270d4}button.danger{background:#b62324}button.danger:hover{background:#8e1b1c}button.ghost{background:#21262d;border:1px solid #30363d}
table{width:100%;border-collapse:separate;border-spacing:0;font-size:14px}td,th{padding:8px;border-bottom:1px solid #21262d;text-align:left;vertical-align:top}
.err{color:#f85149;min-height:20px}.ok{color:#3fb950}.muted{color:#8b949e;font-size:13px}
.center{max-width:380px;margin:12vh auto}
.row{display:flex;gap:16px;flex-wrap:wrap}.row .card{flex:1;min-width:240px}
.stat{font-size:28px;font-weight:600}
nav{display:flex;gap:8px;flex-wrap:wrap;margin-bottom:16px}
nav button{background:#21262d;border:1px solid #30363d}nav button.active{background:#1f6feb;border-color:#1f6feb}
section{display:none}section.active{display:block}
.dl{display:flex;gap:8px;align-items:center}.dl input{margin:0;min-width:0;flex:1}
table.gent{table-layout:fixed}
td.fname{word-break:break-all;font-size:12px;line-height:1.5}
td.nowrap{white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
/* v43: responsive tables (modern-web-guidance responsive-table +
   single-line-overflow guides). Every console table sits in a
   .table-wrapper: a scroll container, keyboard-focusable via
   tabindex=0, that is also the container-query scope. Headers stick
   while scrolling; under 600px the table becomes stacked cards with
   labels injected from --label-N variables (set per table in JS).
   Long cells truncate with an ellipsis; the full text stays in the
   DOM, shows via the title attribute on hover, and on narrow (touch)
   screens the stacked layout shows it in full — the wrapper's
   horizontal scroll is the other route to content that does not fit. */
.table-wrapper{overflow:auto;max-inline-size:100%;max-block-size:min(500px,80vh);container-type:inline-size}
.table-wrapper table{table-layout:fixed}
thead{position:sticky;inset-block-start:0;z-index:1}
thead th{position:sticky;inset-block-start:0;background:#1c2128}
tbody th{position:sticky;inset-inline-start:0;background:#161b22}
td.trunc{white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.table-wrapper table.kv td{overflow-wrap:anywhere}
@container (width < 600px){
.table-wrapper table{min-width:0!important}
.table-wrapper table,.table-wrapper thead,.table-wrapper tbody,.table-wrapper tr,.table-wrapper th,.table-wrapper td{display:block}
.table-wrapper thead{display:none}
.table-wrapper tr{margin-block-end:1.5rem;border:1px solid #30363d}
.table-wrapper tbody th{position:sticky;inset-block-start:0;z-index:2;background:#1c2128}
.table-wrapper td.trunc,.table-wrapper td.nowrap{white-space:normal;overflow:visible;text-overflow:clip}
.table-wrapper td:nth-child(1)::before{content:var(--label-1) ": ";font-weight:600;color:#8b949e}
.table-wrapper td:nth-child(2)::before{content:var(--label-2) ": ";font-weight:600;color:#8b949e}
.table-wrapper td:nth-child(3)::before{content:var(--label-3) ": ";font-weight:600;color:#8b949e}
.table-wrapper td:nth-child(4)::before{content:var(--label-4) ": ";font-weight:600;color:#8b949e}
.table-wrapper td:nth-child(5)::before{content:var(--label-5) ": ";font-weight:600;color:#8b949e}
.table-wrapper td:nth-child(6)::before{content:var(--label-6) ": ";font-weight:600;color:#8b949e}
.table-wrapper td:nth-child(7)::before{content:var(--label-7) ": ";font-weight:600;color:#8b949e}
.table-wrapper td:nth-child(8)::before{content:var(--label-8) ": ";font-weight:600;color:#8b949e}
}
code{background:#0d1117;padding:2px 6px;border-radius:4px;font-size:13px}
.cap{display:flex;gap:8px;align-items:center}.cap img{height:54px;border:1px solid #30363d;border-radius:6px;background:#e8ecf1;display:block}.cap input{margin:0}
.capbtn{background:transparent;border:0;padding:0;cursor:pointer}.capbtn:hover{background:transparent}
.capq{font-size:18px;font-weight:600;white-space:nowrap}
.linkbtn{background:none;border:0;color:#58a6ff;padding:0;font-size:13px;cursor:pointer;text-decoration:underline}.linkbtn:hover{background:none}
.changelog{white-space:pre-wrap;word-break:break-word;max-height:280px;overflow-y:auto;background:#0d1117;border:1px solid #30363d;border-radius:6px;padding:10px;font-size:13px;line-height:1.6}
.langrow{display:flex;gap:6px;justify-content:flex-end;margin-bottom:12px}
button.langbtn.active{background:#1f6feb;border-color:#1f6feb}
.badge{display:inline-block;background:#1f6feb33;border:1px solid #1f6feb;color:#58a6ff;border-radius:4px;padding:1px 6px;font-size:12px;margin-left:6px;white-space:nowrap}
"#;

// Shared i18n dictionary + helpers, injected into every page by page().
// Placeholders {n} / {v} are replaced by the caller with .replace().
// NOTE: never introduce a double-quote immediately followed by a hash
// in here: it would terminate the Rust raw string. JS strings use
// single quotes and contain no ASCII apostrophes.
const I18N_JS: &str = r#"<script>
const I18N={zh:{
'app.title':'RustDesk 服务控制台','app.logout':'退出','app.loading':'加载中…',
'app.load_failed':'加载失败','app.colon':'：','app.period':'。','app.retry':'重试',
'app.po':'（','app.pc':'）',
'app.unknown_err':'未知错误','app.network_err':'网络错误，请重试','app.op_failed':'操作失败',
'app.failed':'失败','app.copy':'复制','app.copied':'已复制','app.copy_failed':'复制失败',
'app.download':'下载','app.delete':'删除','app.cancel':'取消',
'app.load_fail_refresh':'加载失败，请检查网络后刷新页面','app.who_admin':'（管理员）',
'auth.username':'用户名','auth.password':'密码','auth.password_min':'密码（至少 6 位）',
'auth.captcha':'验证码','auth.cap_img_ph':'输入图片中的字符','auth.cap_text_ph':'输入计算结果',
'auth.cap_refresh':'点击刷新验证码','auth.cap_aria':'刷新验证码','auth.cap_alt':'图片验证码','auth.cap_loading':'加载中…',
'auth.cap_load_fail':'加载失败，请重试','auth.cap_to_text':'看不清？改用文字题',
'auth.cap_to_img':'改用图片验证码','auth.signin':'登录','auth.signup':'注册',
'auth.signin_fail':'登录失败','auth.signup_fail':'注册失败','auth.signup_title':'注册账号',
'auth.no_account':'没有账号？','auth.first_admin':'（首个注册用户自动成为管理员）',
'auth.has_account':'已有账号？','auth.network_err':'网络错误，请检查连接后重试',
'auth.login_page_title':'登录 - RustDesk 服务控制台','auth.register_page_title':'注册 - RustDesk 服务控制台',
'auth.oidc_title':'登录 RustDesk','auth.oidc_desc':'使用你的账号登录，登录后请返回 RustDesk 客户端。',
'auth.oidc_ok_title':'登录成功','auth.oidc_ok_desc':'请返回 RustDesk 客户端，登录状态会自动完成。可以关闭此页面。',
'pwd.title':'修改密码','pwd.old':'旧密码','pwd.new':'新密码','pwd.confirm':'确认新密码',
'pwd.old_ph':'当前使用的密码','pwd.new_ph':'新密码（至少 6 位）','pwd.confirm_ph':'再次输入新密码',
'pwd.submit':'确认修改','pwd.ok':'密码已修改。其他设备上的登录已失效，请用新密码重新登录。',
'pwd.mismatch':'两次输入的新密码不一致','pwd.empty_old':'请输入旧密码','pwd.short':'新密码至少 6 位',
'nav.overview':'概览','nav.devices':'设备','nav.audit':'审计日志','nav.users':'用户管理','nav.tokens':'API Token',
'nav.genclient':'生成客户端','nav.settings':'服务器设置',
'ov.users':'注册用户','ov.devices':'已绑定设备','ov.peers':'已注册终端（hbbs）',
'ov.about':'说明','ov.about_text':'客户端在「设置 → 网络」填入本服务器地址，或直接使用「生成客户端」里预绑定配置的安装包，即可配合账号登录使用。设备在客户端登录账号后会通过心跳自动绑定到所属用户。',
'ov.load_fail':'概览加载失败，请刷新重试','ov.load_fail2':'概览加载失败：',
'dev.title':'设备','dev.refresh':'刷新','dev.updated':'更新于 ',
'dev.empty':'暂无设备。设备在客户端登录账号后会出现在这里。','dev.id':'设备 ID',
'dev.status':'状态','dev.hostname':'主机名','dev.os':'系统','dev.owner':'所属用户',
'dev.version':'版本','dev.last_seen':'最近在线','dev.last_relay':'最近使用的中继','dev.online':'● 在线','dev.offline':'● 离线',
'dev.unbound':'未绑定',
'usr.title':'用户管理','usr.username':'用户名','usr.role':'角色','usr.status':'状态',
'usr.created':'注册时间','usr.actions':'操作','usr.admin':'管理员','usr.regular':'普通',
'usr.disabled':'已禁用','usr.active':'正常','usr.enable':'启用','usr.disable':'禁用',
'usr.unadmin':'取消管理员','usr.make_admin':'设管理员','usr.reset_pwd':'重置密码',
'usr.delete_confirm':'删除用户 {n}？该账号将无法登录，其设备变为未绑定，名下 API Token 同时吊销；删除不可恢复。',
'usr.pwd_prompt':'为 {n} 设置新密码（至少 6 位）','usr.pwd_reset':'已重置',
'usr.create':'新建用户','usr.create_submit':'创建','usr.password':'密码','usr.new_name_ph':'用户名（3-32 位字母、数字、_-.)','usr.new_pass_ph':'密码（至少 6 位）','usr.created_ok':'用户已创建',
'tok.title':'API Token（供第三方系统集成）','tok.desc1':'请求带',
'tok.desc2':'即可调用管理接口。Token 只在创建时显示一次。','tok.name':'名称',
'tok.name_ph':'如：内部系统','tok.create':'创建 Token','tok.created':'创建时间',
'tok.last_used':'最近使用','tok.revoke':'吊销','tok.empty':'暂无 Token',
'tok.new_label':'新 Token（只显示一次，请保存）：',
'tok.revoke_confirm':'吊销 Token「{n}」？使用该 Token 的程序将立即无法访问，吊销不可恢复。',
'tok.revoke_failed':'吊销失败',
'gen.title':'生成预绑定客户端',
'gen.desc':'生成的安装包已内置本服务器地址与密钥（编码进文件名，界面不显示服务器与密钥设置）。把下载链接发给用户即可，用户安装后直接登录账号使用。',
'gen.label':'备注（可选）','gen.label_ph':'如：公司办公电脑','gen.btn':'生成 Windows 客户端',
'gen.file':'文件','gen.remark':'备注','gen.by':'生成者','gen.time':'生成时间',
'gen.link':'下载链接',
'gen.empty_admin':'尚未生成。先在「服务器设置」里填公网地址并下载模板。',
'gen.empty_user':'尚未生成。生成前需管理员先在服务器设置里填公网地址并下载模板。',
'gen.failed':'生成失败',
'gen.delete_confirm':'删除生成的客户端「{n}」？删除后，已发出的下载链接立即失效，且不可恢复。',
'gen.delete_failed':'删除失败',
'cfg.title':'官方客户端手动配置',
'cfg.desc':'已经装好官方 RustDesk 客户端的用户不用下载上面的安装包：在官方客户端「设置 → 网络」里填入下面四项，保存后即可登录本服务器的账号使用。下面显示的值与生成客户端内置的配置完全一致。',
'cfg.unconfigured_admin':'尚未在「服务器设置」里填写公网地址，填好后这里会显示客户端配置。',
'cfg.unconfigured_user':'管理员尚未填写服务器公网地址，暂时无法显示客户端配置。',
'cfg.host':'ID 服务器','cfg.relay':'中继服务器','cfg.api':'API 服务器','cfg.key':'密钥（Key）',
'cfg.copy_all':'一键复制配置',
'cfg.copy_hint':'复制后，在官方客户端「设置 → 网络」点粘贴图标即可导入整份配置，无需逐项填写。',
'dl.title':'其他平台下载',
'dl.desc':'其他平台请下载官方 RustDesk 客户端，安装后按上方「官方客户端手动配置」填写服务器地址与密钥，即可登录本服务器的账号使用。默认显示官方直链；勾选「镜像加速」后切换为经 GitHub 镜像下载的加速链接（iOS 为 App Store 直链，不受影响）。',
'dl.mirror':'镜像加速','dl.empty':'暂无可下载的版本。','dl.latest':'官方最新版本：',
'dl.platform':'平台','dl.file':'文件','dl.download':'下载','dl.official':'官方下载',
'dl.mirror_link':'加速下载','dl.appstore':'App Store',
'set.title':'服务器设置','set.host_label':'服务器公网地址（域名或 IP，不要带端口，生成客户端必填）',
'set.host_ph':'如 rd.example.com 或 203.0.113.10',
'set.api_label':'API 地址覆盖（可空，默认 http://公网地址:21114）','set.api_ph':'可空',
'set.mirror_label':'GitHub 加速镜像前缀（可空，下载官方客户端模板时优先使用；内置多个公共镜像自动轮询，最后直连）',
'set.mirror_ph':'如 https://gh-proxy.com',
'set.hide':'在客户端隐藏服务器/密钥设置（通过心跳下发）',
'set.allowreg':'允许开放注册（任何人可自助注册账号）',
'set.allowreg_note':'关闭后，任何人无法自助注册；管理员在「用户管理」可建号。公网部署建议关闭。',
'set.allowrc':'允许远程修改配置（通过心跳下发到所有客户端）',
'set.allowrc_note':'开启后，控制端可以操作被控端的 RustDesk 客户端界面与设置（包括改密码、改配置），不用再逐台到被控端本地勾选。仅在自有设备上开启；已在本机手动改过这一项的客户端以本机设置为准。',
'set.mustlogin':'必须登录（所有客户端必须登录后才能连接）',
'set.mustlogin_note':'开启后，未登录的客户端（没有账号登录或登录已失效）无法发起任何连接，包括直连与中继；纯 ID+Key（无账号）部署请勿开启。',
'set.save':'保存设置','set.saved':'已保存','set.load_fail':'加载设置失败，请刷新重试',
'set.load_fail2':'加载设置失败：','set.ports_title':'监听端口（当前生效值）',
'set.ports_loading':'读取中…',
'set.ports_desc':'端口由启动参数决定，不在这里改：编辑程序目录下的 server.env（没有就从 server.env.example 复制一份），改 HBBS_PORT / HBBR_PORT / API_PORT / RELAY_SERVERS 后重启服务端。改端口后必须重新生成客户端（新客户端才会带新端口），并更新路由器端口转发；已生成的旧客户端会连不上新端口。换端口只能躲开针对默认端口的批量扫描，真正的安全仍靠服务器密钥与账号登录——知道新端口的人仍可注册 ID。',
'set.ports_rendezvous':'中继/ID：','set.ports_nat':'（NAT 测试 ','set.ports_ws':'，WebSocket ',
'set.ports_relay':'）｜中继：','set.ports_api':'）｜API：','set.ports_console':'｜控制台（独立监听）：',
'set.ports_same':'（与 API 同端口）','set.ports_tls':'｜HTTPS：已启用',
'set.svc_title':'系统服务（Ubuntu）',
'set.svc_desc':'在程序目录执行下面命令，把服务端注册为 systemd 服务：开机自启、崩溃自动重启。服务跑的就是 start.sh，所以 server.env 里的端口与中继配置同样生效；升级时只需替换 hbbs / hbbr 两个文件，不用重装服务。',
'set.svc_remove':'# 移除服务','set.svc_logs':'# 看服务日志',
'set.tpl_title':'官方客户端模板','set.tpl_checking':'检查中…',
'set.tpl_ver_label':'版本（可空 = 最新版）','set.tpl_ver_ph':'如 1.4.2',
'set.tpl_btn':'下载/更新模板',
'set.tpl_desc':'模板是官方 RustDesk Windows 安装程序，生成客户端时会在其基础上改文件名注入配置，不修改程序本身。下载会自动在多个 GitHub 加速镜像与直连之间切换（某个源太慢会自动换下一个）。',
'set.tpl_ready':'已就绪：','set.tpl_not_dl':'未下载（生成客户端前需要先下载）',
'set.tpl_downloading':'正在下载官方客户端模板','set.tpl_speed':' · 速度 ',
'set.tpl_source':' · 来源：','set.tpl_done':'下载完成','set.tpl_failed':'下载失败：',
'set.tpl_start_failed':'启动下载失败：','set.tpl_downloading2':'正在下载…','set.tpl_uptodate':'已是最新版本（{v}），无需下载',
'set.upd_title':'服务端版本更新','set.upd_loading':'读取中…','set.upd_check':'检测更新',
'set.upd_desc':'更新包来自本项目的 GitHub Releases，按当前平台自动选择。GitHub 加速用的是上方「GitHub 加速镜像前缀」设置，未配置时用内置镜像链。下载完成后新文件先暂存，点「立即重启」后由启动器（rdsvc / start.sh / start.ps1 / systemd）在重启时替换生效；直接运行 hbbs（无启动器托管）时，需手动把 hbbs.new / hbbr.new 改名覆盖原文件后再启动。',
'set.upd_load_fail':'版本信息加载失败，请刷新重试','set.upd_load_fail2':'版本信息加载失败：',
'set.upd_current':'当前版本：','set.upd_base':'（上游基底 ','set.upd_platform':'）｜平台：',
'set.upd_staged':'｜已暂存 ','set.upd_staged2':'，重启后生效','set.upd_check_fail':'检测失败',
'set.upd_latest':'已是最新版本（最新：','set.upd_latest2':'）。',
'set.upd_notes_na':'更新日志暂不可用（本次检测未能获取到更新说明，可稍后重试）。',
'set.upd_no_notes':'暂无更新说明。','set.upd_found':'发现新版本：',
'set.upd_released':'（发布于 ','set.upd_to':'更新到 ',
'set.upd_confirm':'确定更新到 {v} 吗？将从 GitHub 下载新版安装包（约 35 MB），下载完成后需重启服务端生效。',
'set.upd_check_netfail':'检测失败：网络错误或检测超时，请重试','set.upd_start_failed':'启动下载失败',
'set.upd_extracting':'正在解压并暂存…','set.upd_failed':'更新失败：',
'set.upd_staged_msg':'已暂存 {v}，重启后生效。','set.upd_restart':'立即重启',
'set.upd_restart_note':'重启后服务会短暂中断，页面会在新版本起来后自动刷新。',
'set.upd_restart_confirm':'确定立即重启服务端以应用 {v} 吗？正在进行的连接会中断。',
'set.upd_restart_failed':'重启失败','set.upd_restarting':'正在重启…等待新版本响应后自动刷新。',
'set.upd_timeout':'等待超时，服务可能未自动恢复。请检查服务端是否在运行（直接运行 hbbs 时需手动启动），然后刷新页面。',
'relay.regpage.title':'中继节点注册','relay.regpage.valid':'注册链接有效。在中继机器的安装目录下运行以下命令完成注册。',
'relay.regpage.used':'此注册链接已被使用。请在控制台重新生成一个注册链接。',
'relay.regpage.expired':'此注册链接已过期。请在控制台重新生成一个注册链接。',
'relay.regpage.invalid':'注册链接无效。请检查链接是否完整，或在控制台重新生成。',
'relay.regpage.cmd_hint':'在中继机器上，进入 relay-agent 与 hbbr 所在目录，运行：',
'relay.regpage.copy':'复制命令','relay.regpage.copied':'已复制',
'relay.regpage.warn':'此链接含注册密钥，相当于凭证：注册完成后即失效，请勿转发或公开。',
'relay.regpage.win_hint':'Windows 上同样在 relay-agent.exe 所在目录运行此命令（PowerShell 或 cmd）。',
'relay.title':'中继节点','relay.badge':'中继',
'relay.desc':'中继节点由常驻代理（relay-agent）管理：代理注册后每 20 秒心跳上报状态，控制台下发的配置随心跳响应加密回传。注意：注册只建立管理关系，不会自动把节点加入中继轮询列表——轮询列表仍由服务器启动参数 RELAY_SERVERS 决定。',
'relay.label':'备注（可选）','relay.label_ph':'如：香港中继','relay.expires':'有效期（小时，1–168）',
'relay.gen':'生成注册链接','relay.gen_fail':'生成失败',
'relay.link_once':'注册链接只显示一次，请立即复制并妥善保管：',
'relay.link_expires':'有效期至 ','relay.cmd_label':'在中继机器上运行：',
'relay.links_title':'待使用的注册链接','relay.links_empty':'暂无待使用的注册链接',
'relay.links_created':'生成时间','relay.links_expires':'过期时间','relay.links_used':'已使用',
'relay.links_revoke':'吊销','relay.links_revoke_confirm':'确定吊销这个注册链接吗？',
'relay.empty':'暂无中继节点','relay.node':'节点','relay.status':'状态','relay.address':'地址',
'relay.last_ip':'上次来源 IP','relay.agent_ver':'代理版本','relay.hbbr_ver':'hbbr 版本',
'relay.hbbr_running':'hbbr 运行中','relay.hbbr_down':'hbbr 未运行','relay.started_at':'hbbr 启动时间',
'relay.last_hb':'最后心跳','relay.sessions':'会话数','relay.throughput':'吞吐',
'relay.key_done':'密钥已交付','relay.key_pending':'密钥未交付','relay.cfg_ver':'配置版本',
'relay.pending_cmd':'待执行命令','relay.detail_back':'← 返回节点列表','relay.cfg_title':'下发配置',
'relay.port':'监听端口','relay.limit_speed':'限速 LIMIT_SPEED（Mb/s）',
'relay.limit_total':'总带宽 TOTAL_BANDWIDTH（Mb/s）','relay.limit_single':'单连接带宽 SINGLE_BANDWIDTH（Mb/s）',
'relay.default_hint':'留空表示 hbbr 默认值（32 / 1024 / 128）','relay.push':'下发配置',
'relay.push_ok':'配置已下发，将在下次心跳（20 秒内）生效。','relay.push_fail':'下发失败',
'relay.restart':'重启 hbbr','relay.restart_confirm':'确定远程重启该节点的 hbbr 吗？该节点上正在进行的中继会话会中断。',
'relay.restart_ok':'重启命令已下发，将在下次心跳执行。','relay.delete':'删除节点',
'relay.delete_confirm':'确定删除节点 {n} 吗？删除后该节点的凭证立即失效，代理将停止心跳（hbbr 本身继续运行，需在中继机器上手动停止）。',
'relay.load_fail':'加载中继节点失败','relay.online':'● 在线','relay.offline':'● 离线',
'relay.unknown_time':'未知','relay.never':'从未',
'relay.port_change_note':'改端口会重启 hbbr（会话中断）；只改限速时即时生效，不重启。',
'aud.title':'审计日志','aud.desc':'被控端上报的连接记录：谁在什么时间从哪里连到哪台设备、是否走中继、连了多久。用户名来自设备绑定，未绑定设备只显示 ID。',
'aud.search_ph':'搜索控制端/被控端 ID、用户名、主机名','aud.search':'搜索','aud.refresh':'刷新',
'aud.range_today':'今天','aud.range_7d':'7 天','aud.range_30d':'30 天','aud.range_all':'全部',
'aud.col_started':'开始时间','aud.col_controller':'控制端','aud.col_controlled':'被控端',
'aud.col_ip':'控制端 IP','aud.col_relay':'中继/直连','aud.col_duration':'时长','aud.col_status':'状态',
'aud.relayed':'中继','aud.direct':'直连','aud.unknown':'—','aud.open':'进行中','aud.closed':'已结束','aud.closed_stale':'（无关闭记录）','aud.closed_offline':'（设备离线）',
'aud.empty':'暂无审计记录。','aud.page':'第 {p} / {n} 页（共 {t} 条）','aud.prev':'上一页','aud.next':'下一页'
},en:{
'app.title':'RustDesk Server Console','app.logout':'Sign out','app.loading':'Loading…',
'app.load_failed':'Load failed','app.colon':': ','app.period':'. ','app.retry':'Retry',
'app.po':'(','app.pc':')',
'app.unknown_err':'Unknown error','app.network_err':'Network error, please retry',
'app.op_failed':'Operation failed','app.failed':'Failed','app.copy':'Copy','app.copied':'Copied',
'app.copy_failed':'Copy failed','app.download':'Download','app.delete':'Delete','app.cancel':'Cancel','app.load_fail_refresh':'Failed to load. Check your network and refresh the page.',
'app.who_admin':' (admin)',
'auth.username':'Username','auth.password':'Password','auth.password_min':'Password (at least 6 characters)',
'auth.captcha':'Captcha','auth.cap_img_ph':'Enter the characters in the image',
'auth.cap_text_ph':'Enter the calculation result','auth.cap_refresh':'Click to refresh the captcha','auth.cap_aria':'Refresh captcha',
'auth.cap_alt':'Image captcha','auth.cap_loading':'Loading…','auth.cap_load_fail':'Failed to load, please retry',
'auth.cap_to_text':'Cannot read it? Switch to a text question','auth.cap_to_img':'Switch back to image captcha',
'auth.signin':'Sign in','auth.signup':'Register','auth.signin_fail':'Sign-in failed',
'auth.signup_fail':'Registration failed','auth.signup_title':'Create Account',
'auth.no_account':'No account? ','auth.first_admin':' (the first registered user becomes admin)',
'auth.has_account':'Already have an account? ','auth.network_err':'Network error. Check your connection and retry.',
'auth.login_page_title':'Sign in - RustDesk Server Console','auth.register_page_title':'Register - RustDesk Server Console',
'auth.oidc_title':'Sign in to RustDesk','auth.oidc_desc':'Sign in with your account, then return to the RustDesk client.',
'auth.oidc_ok_title':'Signed in','auth.oidc_ok_desc':'Return to the RustDesk client; the sign-in will complete automatically. You can close this page.',
'pwd.title':'Change Password','pwd.old':'Current password','pwd.new':'New password','pwd.confirm':'Confirm new password',
'pwd.old_ph':'Your current password','pwd.new_ph':'New password (at least 6 characters)','pwd.confirm_ph':'Enter the new password again',
'pwd.submit':'Confirm change','pwd.ok':'Password changed. Other devices have been signed out; sign in again with the new password.',
'pwd.mismatch':'The new passwords do not match','pwd.empty_old':'Enter the current password','pwd.short':'The new password must be at least 6 characters',
'nav.overview':'Overview','nav.devices':'Devices','nav.audit':'Audit Log','nav.users':'User Management','nav.tokens':'API Tokens',
'nav.genclient':'Generate Client','nav.settings':'Server Settings',
'ov.users':'Registered users','ov.devices':'Bound devices','ov.peers':'Registered endpoints (hbbs)',
'ov.about':'About','ov.about_text':'In the client, fill in this server address under Settings → Network, or use a pre-configured installer from Generate Client, then sign in with an account. Devices are bound to their owner automatically via heartbeat after signing in.',
'ov.load_fail':'Failed to load the overview. Refresh and retry.','ov.load_fail2':'Failed to load the overview: ',
'dev.title':'Devices','dev.refresh':'Refresh','dev.updated':'Updated at ',
'dev.empty':'No devices yet. Devices appear here after signing in with an account in the client.',
'dev.id':'Device ID','dev.status':'Status','dev.hostname':'Hostname','dev.os':'OS',
'dev.owner':'Owner','dev.version':'Version','dev.last_seen':'Last seen','dev.last_relay':'Last relay used','dev.online':'● Online',
'dev.offline':'● Offline','dev.unbound':'Unbound',
'usr.title':'User Management','usr.username':'Username','usr.role':'Role','usr.status':'Status',
'usr.created':'Registered','usr.actions':'Actions','usr.admin':'Admin','usr.regular':'Regular',
'usr.disabled':'Disabled','usr.active':'Active','usr.enable':'Enable','usr.disable':'Disable',
'usr.unadmin':'Remove admin','usr.make_admin':'Make admin','usr.reset_pwd':'Reset password',
'usr.delete_confirm':'Delete user {n}? The account will no longer be able to sign in, its devices will become unbound, and its API tokens will be revoked. This cannot be undone.',
'usr.pwd_prompt':'Set a new password for {n} (at least 6 characters)','usr.pwd_reset':'Password reset',
'usr.create':'Create user','usr.create_submit':'Create','usr.password':'Password','usr.new_name_ph':'Username (3-32 letters, digits, _-.)','usr.new_pass_ph':'Password (at least 6 characters)','usr.created_ok':'User created',
'tok.title':'API Tokens (for third-party integrations)','tok.desc1':'Send requests with',
'tok.desc2':'to call the management APIs. A token is shown only once, when created.',
'tok.name':'Name','tok.name_ph':'e.g. internal system','tok.create':'Create Token',
'tok.created':'Created','tok.last_used':'Last used','tok.revoke':'Revoke','tok.empty':'No tokens yet',
'tok.new_label':'New token (shown only once, please save it):',
'tok.revoke_confirm':'Revoke token “{n}”? Applications using it will immediately lose access. This cannot be undone.',
'tok.revoke_failed':'Failed to revoke',
'gen.title':'Generate Pre-configured Client',
'gen.desc':'The generated installer has this server address and key built in (encoded in the file name; the server and key settings are hidden in the client UI). Share the download link with users; they can sign in with their account right after installing.',
'gen.label':'Remark (optional)','gen.label_ph':'e.g. office PC','gen.btn':'Generate Windows Client',
'gen.file':'File','gen.remark':'Remark','gen.by':'Created by','gen.time':'Created',
'gen.link':'Download link',
'gen.empty_admin':'Nothing generated yet. Fill in the public address in Server Settings and download the template first.',
'gen.empty_user':'Nothing generated yet. An admin must fill in the public address in Server Settings and download the template first.',
'gen.failed':'Generation failed',
'gen.delete_confirm':'Delete the generated client “{n}”? Issued download links will stop working immediately. This cannot be undone.',
'gen.delete_failed':'Failed to delete',
'cfg.title':'Manual Configuration for Official Client',
'cfg.desc':'Users who already have the official RustDesk client installed do not need the installer above: fill in the four fields below in the official client under Settings → Network, save, and sign in with an account on this server. The values shown here are identical to the configuration built into generated clients.',
'cfg.unconfigured_admin':'The public address has not been filled in under Server Settings; the client configuration will appear here once it is set.',
'cfg.unconfigured_user':'The admin has not filled in the public server address, so the client configuration cannot be shown yet.',
'cfg.host':'ID Server','cfg.relay':'Relay Server','cfg.api':'API Server','cfg.key':'Key',
'cfg.copy_all':'Copy Configuration',
'cfg.copy_hint':'After copying, tap the paste icon in Settings → Network of the official client to import the whole configuration; no need to fill in each field.',
'dl.title':'Other Platform Downloads',
'dl.desc':'For other platforms, download the official RustDesk client, install it, then fill in the server address and key from “Manual Configuration for Official Client” above to sign in with an account on this server. Direct links are shown by default; check “Use mirror” to switch to accelerated links via a GitHub mirror (iOS uses the App Store link and is unaffected).',
'dl.mirror':'Use mirror','dl.empty':'No downloadable version available.',
'dl.latest':'Latest official version: ','dl.platform':'Platform','dl.file':'File',
'dl.download':'Download','dl.official':'Official download','dl.mirror_link':'Mirror download',
'dl.appstore':'App Store',
'set.title':'Server Settings','set.host_label':'Public server address (domain or IP, no port; required for generating clients)',
'set.host_ph':'e.g. rd.example.com or 203.0.113.10',
'set.api_label':'API URL override (optional; default http://<public address>:21114)','set.api_ph':'Optional',
'set.mirror_label':'GitHub mirror prefix (optional; used first when downloading the official client template; built-in public mirrors are tried automatically, then direct)',
'set.mirror_ph':'e.g. https://gh-proxy.com',
'set.hide':'Hide server/key settings in clients (pushed via heartbeat)',
'set.allowreg':'Allow open registration (anyone can self-register)',
'set.allowreg_note':'When off, nobody can self-register; admins can create accounts in User Management. Recommended off for public deployments.',
'set.allowrc':'Allow remote configuration changes (pushed to all clients via heartbeat)',
'set.allowrc_note':'When on, controllers can operate the controlled side’s RustDesk client UI and settings (including changing passwords and configuration) without enabling it locally on each machine. Only enable this on your own devices; clients where this was set manually keep their local setting.',
'set.mustlogin':'Require login (all clients must log in before connecting)',
'set.mustlogin_note':'When on, clients that are not logged in (no account login, or login no longer valid) cannot start any connection, direct or relayed. Do not enable this on ID+key-only deployments without accounts.',
'set.save':'Save Settings','set.saved':'Saved','set.load_fail':'Failed to load settings. Refresh and retry.',
'set.load_fail2':'Failed to load settings: ','set.ports_title':'Listening Ports (current values)',
'set.ports_loading':'Loading…',
'set.ports_desc':'Ports are decided by startup arguments, not changed here: edit server.env in the program directory (copy server.env.example if there is none), change HBBS_PORT / HBBR_PORT / API_PORT / RELAY_SERVERS, then restart the server. After changing ports you must regenerate clients (only new clients carry the new ports) and update router port forwarding; previously generated clients cannot reach the new ports. Changing ports only dodges bulk scans aimed at the default ports; real security still comes from the server key and account sign-in — anyone who knows the new port can still register an ID.',
'set.ports_rendezvous':'Relay/ID: ','set.ports_nat':' (NAT test ','set.ports_ws':', WebSocket ',
'set.ports_relay':') | Relay: ','set.ports_api':') | API: ','set.ports_console':' | Console (separate listener): ',
'set.ports_same':' (same port as API)','set.ports_tls':' | HTTPS: enabled',
'set.svc_title':'System Service (Ubuntu)',
'set.svc_desc':'Run the commands below in the program directory to register the server as a systemd service: auto-start on boot, auto-restart on crash. The service runs start.sh, so the ports and relay settings in server.env apply; when upgrading, just replace the hbbs / hbbr binaries — no need to reinstall the service.',
'set.svc_remove':'# Remove the service','set.svc_logs':'# View service logs',
'set.tpl_title':'Official Client Template','set.tpl_checking':'Checking…',
'set.tpl_ver_label':'Version (empty = latest)','set.tpl_ver_ph':'e.g. 1.4.2',
'set.tpl_btn':'Download/Update Template',
'set.tpl_desc':'The template is the official RustDesk Windows installer; generating a client injects the configuration by renaming it, without modifying the program itself. Downloads automatically switch between several GitHub mirrors and direct access (a slow source is skipped for the next one).',
'set.tpl_ready':'Ready: ','set.tpl_not_dl':'Not downloaded (download it before generating clients)',
'set.tpl_downloading':'Downloading the official client template','set.tpl_speed':' · Speed ',
'set.tpl_source':' · Source: ','set.tpl_done':'Download complete','set.tpl_failed':'Download failed: ',
'set.tpl_start_failed':'Failed to start download: ','set.tpl_downloading2':'Downloading…','set.tpl_uptodate':'Already the latest version ({v}), no download needed',
'set.upd_title':'Server Update','set.upd_loading':'Loading…','set.upd_check':'Check for Updates',
'set.upd_desc':'Update packages come from this project’s GitHub Releases and are selected automatically for the current platform. GitHub acceleration uses the “GitHub mirror prefix” setting above, or the built-in mirror chain when unset. After downloading, the new files are staged; clicking “Restart Now” lets the launcher (rdsvc / start.sh / start.ps1 / systemd) replace them during restart. If you run hbbs directly (no launcher), rename hbbs.new / hbbr.new over the original files manually before starting.',
'set.upd_load_fail':'Failed to load version info. Refresh and retry.','set.upd_load_fail2':'Failed to load version info: ',
'set.upd_current':'Current version: ','set.upd_base':' (upstream base ','set.upd_platform':') | Platform: ',
'set.upd_staged':' | Staged ','set.upd_staged2':', takes effect after restart','set.upd_check_fail':'Detection failed',
'set.upd_latest':'Already up to date (latest: ','set.upd_latest2':').',
'set.upd_notes_na':'Release notes unavailable (could not be fetched this time; please retry later).',
'set.upd_no_notes':'No release notes.','set.upd_found':'New version available: ',
'set.upd_released':' (released ','set.upd_to':'Update to ',
'set.upd_confirm':'Update to {v}? The new package (about 35 MB) will be downloaded from GitHub, and the server must be restarted to apply it.',
'set.upd_check_netfail':'Detection failed: network error or timeout, please retry','set.upd_start_failed':'Failed to start download',
'set.upd_extracting':'Extracting and staging…','set.upd_failed':'Update failed: ',
'set.upd_staged_msg':'Staged {v}; it will take effect after restart.','set.upd_restart':'Restart Now',
'set.upd_restart_note':'The service will be briefly interrupted during the restart; the page will refresh automatically once the new version is up.',
'set.upd_restart_confirm':'Restart the server now to apply {v}? Active connections will be interrupted.',
'set.upd_restart_failed':'Restart failed','set.upd_restarting':'Restarting… waiting for the new version to respond, then refreshing automatically.',
'set.upd_timeout':'Timed out waiting; the service may not have recovered automatically. Check whether the server is running (start hbbs manually if you run it directly), then refresh the page.',
'relay.regpage.title':'Relay Node Registration','relay.regpage.valid':'This registration link is valid. Run the following command in the relay installation directory to complete registration.',
'relay.regpage.used':'This registration link has already been used. Please generate a new one in the console.',
'relay.regpage.expired':'This registration link has expired. Please generate a new one in the console.',
'relay.regpage.invalid':'This registration link is invalid. Check the link, or generate a new one in the console.',
'relay.regpage.cmd_hint':'On the relay host, enter the directory containing relay-agent and hbbr, then run:',
'relay.regpage.copy':'Copy command','relay.regpage.copied':'Copied',
'relay.regpage.warn':'This link contains a registration secret — treat it as a credential. It expires once registration completes; do not forward or publish it.',
'relay.regpage.win_hint':'On Windows, run the same command in the directory containing relay-agent.exe (PowerShell or cmd).',
'relay.title':'Relay Nodes','relay.badge':'Relay',
'relay.desc':'Relay nodes are managed by a resident agent (relay-agent): after registration the agent heartbeats every 20 s, and configuration pushed from the console is returned encrypted in the heartbeat response. Note: registration only establishes management; it does not add the node to the relay rotation list — rotation is still decided by the server startup parameter RELAY_SERVERS.',
'relay.label':'Label (optional)','relay.label_ph':'e.g. Hong Kong relay','relay.expires':'Validity (hours, 1–168)',
'relay.gen':'Generate registration link','relay.gen_fail':'Failed to generate',
'relay.link_once':'The registration link is shown only once — copy it now and keep it safe:',
'relay.link_expires':'Valid until ','relay.cmd_label':'Run on the relay host:',
'relay.links_title':'Unused registration links','relay.links_empty':'No unused registration links',
'relay.links_created':'Created','relay.links_expires':'Expires','relay.links_used':'Used',
'relay.links_revoke':'Revoke','relay.links_revoke_confirm':'Revoke this registration link?',
'relay.empty':'No relay nodes yet','relay.node':'Node','relay.status':'Status','relay.address':'Address',
'relay.last_ip':'Last source IP','relay.agent_ver':'Agent version','relay.hbbr_ver':'hbbr version',
'relay.hbbr_running':'hbbr running','relay.hbbr_down':'hbbr not running','relay.started_at':'hbbr started at',
'relay.last_hb':'Last heartbeat','relay.sessions':'Sessions','relay.throughput':'Throughput',
'relay.key_done':'Key delivered','relay.key_pending':'Key not delivered','relay.cfg_ver':'Config version',
'relay.pending_cmd':'Pending command','relay.detail_back':'← Back to node list','relay.cfg_title':'Push configuration',
'relay.port':'Listen port','relay.limit_speed':'Speed limit LIMIT_SPEED (Mb/s)',
'relay.limit_total':'Total bandwidth TOTAL_BANDWIDTH (Mb/s)','relay.limit_single':'Per-connection bandwidth SINGLE_BANDWIDTH (Mb/s)',
'relay.default_hint':'Leave empty for the hbbr default (32 / 1024 / 128)','relay.push':'Push configuration',
'relay.push_ok':'Configuration pushed; it takes effect at the next heartbeat (within 20 s).','relay.push_fail':'Failed to push',
'relay.restart':'Restart hbbr','relay.restart_confirm':'Restart hbbr on this node? Active relay sessions on the node will drop.',
'relay.restart_ok':'Restart command queued; it runs at the next heartbeat.','relay.delete':'Delete node',
'relay.delete_confirm':'Delete node {n}? Its credential is revoked immediately and the agent stops heartbeating (hbbr itself keeps running — stop it on the relay host if needed).',
'relay.load_fail':'Failed to load relay nodes','relay.online':'● Online','relay.offline':'● Offline',
'relay.unknown_time':'Unknown','relay.never':'Never',
'relay.port_change_note':'Changing the port restarts hbbr (sessions drop); changing only the limits applies live without a restart.',
'aud.title':'Audit Log','aud.desc':'Connection records reported by controlled devices: who connected to which device, when, from where, relayed or direct, and for how long. Usernames come from device binding; unbound devices show only their ID.',
'aud.search_ph':'Search controller/controlled ID, username, hostname','aud.search':'Search','aud.refresh':'Refresh',
'aud.range_today':'Today','aud.range_7d':'7 days','aud.range_30d':'30 days','aud.range_all':'All',
'aud.col_started':'Started','aud.col_controller':'Controller','aud.col_controlled':'Controlled',
'aud.col_ip':'Controller IP','aud.col_relay':'Relay/Direct','aud.col_duration':'Duration','aud.col_status':'Status',
'aud.relayed':'Relay','aud.direct':'Direct','aud.unknown':'—','aud.open':'In progress','aud.closed':'Ended','aud.closed_stale':'(no close record)','aud.closed_offline':'(device offline)',
'aud.empty':'No audit records yet.','aud.page':'Page {p} / {n} ({t} total)','aud.prev':'Previous','aud.next':'Next'
}};
function curLang(){try{const l=localStorage.getItem('rdp-lang');return l==='en'?'en':'zh';}catch(e){return 'zh';}}
function t(k){const d=I18N[curLang()]||I18N.zh;if(d[k]!=null)return d[k];if(I18N.zh[k]!=null)return I18N.zh[k];return k;}
function applyI18n(){
  const l=curLang();document.documentElement.lang=l;
  document.querySelectorAll('[data-i18n]').forEach(el=>{const v=t(el.getAttribute('data-i18n'));if(v)el.textContent=v;});
  document.querySelectorAll('[data-i18n-ph]').forEach(el=>{el.placeholder=t(el.getAttribute('data-i18n-ph'));});
  document.querySelectorAll('[data-i18n-title]').forEach(el=>{el.title=t(el.getAttribute('data-i18n-title'));});
  document.querySelectorAll('[data-i18n-aria]').forEach(el=>{el.setAttribute('aria-label',t(el.getAttribute('data-i18n-aria')));});
  document.querySelectorAll('[data-i18n-alt]').forEach(el=>{el.alt=t(el.getAttribute('data-i18n-alt'));});
  document.querySelectorAll('.langbtn').forEach(b=>b.classList.toggle('active',b.getAttribute('data-lang')===l));
  if(window.__pageTitleKey){const ti=t(window.__pageTitleKey);if(ti&&ti!==window.__pageTitleKey)document.title=ti;}
}
function setLang(l){try{localStorage.setItem('rdp-lang',l==='en'?'en':'zh');}catch(e){}applyI18n();if(window.__onLangChange)window.__onLangChange();}
document.addEventListener('click',e=>{const b=e.target.closest?e.target.closest('.langbtn'):null;if(b)setLang(b.getAttribute('data-lang'));});
document.addEventListener('DOMContentLoaded',applyI18n);
</script>"#;

fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"zh\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{title}</title><style>{CSS}</style></head><body>{I18N_JS}{body}</body></html>"
    )
}

pub fn login_page() -> String {
    page(
        "登录 - RustDesk 服务控制台",
        r#"<div class="center"><div class="langrow"><button type="button" class="langbtn ghost" data-lang="zh">中文</button><button type="button" class="langbtn ghost" data-lang="en">English</button></div><div class="card"><h1 data-i18n="app.title">RustDesk 服务控制台</h1>
<div class="err" id="err"></div>
<label data-i18n="auth.username">用户名</label><input id="u" autocomplete="username">
<label data-i18n="auth.password">密码</label><input id="p" type="password" autocomplete="current-password">
<label data-i18n="auth.captcha">验证码</label><div class="cap"><input id="c" autocomplete="off" placeholder="输入图片中的字符" data-i18n-ph="auth.cap_img_ph"><button type="button" id="capbtn" class="capbtn" title="点击刷新验证码" data-i18n-title="auth.cap_refresh" aria-label="刷新验证码" data-i18n-aria="auth.cap_aria"><img id="cap" src="/api/captcha" alt="图片验证码" data-i18n-alt="auth.cap_alt"></button><span id="capq" class="capq" style="display:none"></span></div>
<p class="muted"><button type="button" id="capmode" class="linkbtn">看不清？改用文字题</button></p>
<button id="btn" style="width:100%" data-i18n="auth.signin">登录</button>
<p class="muted"><span data-i18n="auth.no_account">没有账号？</span><a href="/console/register" data-i18n="auth.signup">注册</a><span data-i18n="auth.first_admin">（首个注册用户自动成为管理员）</span></p>
</div></div>
<script>
window.__pageTitleKey='auth.login_page_title';
const cap=document.getElementById('cap');
const capq=document.getElementById('capq');
let textMode=false;
async function loadTextCap(){capq.textContent=t('auth.cap_loading');try{const r=await fetch('/api/captcha?mode=text');const j=await r.json();capq.textContent=j.question||t('auth.cap_load_fail');}catch(e){capq.textContent=t('auth.cap_load_fail');}}
function refreshCap(){if(textMode){loadTextCap();}else{cap.src='/api/captcha?t='+Date.now();}}
document.getElementById('capbtn').onclick=refreshCap;
function syncCapMode(){document.getElementById('c').placeholder=textMode?t('auth.cap_text_ph'):t('auth.cap_img_ph');document.getElementById('capbtn').style.display=textMode?'none':'';capq.style.display=textMode?'':'none';document.getElementById('capmode').textContent=textMode?t('auth.cap_to_img'):t('auth.cap_to_text');}
window.__onLangChange=syncCapMode;
document.getElementById('capmode').onclick=e=>{e.preventDefault();textMode=!textMode;document.getElementById('c').value='';syncCapMode();refreshCap();};
document.getElementById('btn').onclick=async()=>{
  const btn=document.getElementById('btn'); if(btn.disabled)return; btn.disabled=true;
  try{
    const r=await fetch('/api/auth/login',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({username:u.value,password:p.value,captcha:c.value})});
    const j=await r.json();
    if(j.ok){location.href='/console';}else{document.getElementById('err').textContent=j.error||t('auth.signin_fail');refreshCap();document.getElementById('c').value='';btn.disabled=false;}
  }catch(e){document.getElementById('err').textContent=t('auth.network_err');btn.disabled=false;}
};
u.addEventListener('keydown',e=>{if(e.key==='Enter')btn.click();});
p.addEventListener('keydown',e=>{if(e.key==='Enter')btn.click();});
c.addEventListener('keydown',e=>{if(e.key==='Enter')btn.click();});
applyI18n();syncCapMode();
</script>"#,
    )
}

pub fn register_page() -> String {
    page(
        "注册 - RustDesk 服务控制台",
        r#"<div class="center"><div class="langrow"><button type="button" class="langbtn ghost" data-lang="zh">中文</button><button type="button" class="langbtn ghost" data-lang="en">English</button></div><div class="card"><h1 data-i18n="auth.signup_title">注册账号</h1>
<div class="err" id="err"></div>
<label data-i18n="auth.username">用户名</label><input id="u" autocomplete="username">
<label data-i18n="auth.password_min">密码（至少 6 位）</label><input id="p" type="password" autocomplete="new-password">
<label data-i18n="auth.captcha">验证码</label><div class="cap"><input id="c" autocomplete="off" placeholder="输入图片中的字符" data-i18n-ph="auth.cap_img_ph"><button type="button" id="capbtn" class="capbtn" title="点击刷新验证码" data-i18n-title="auth.cap_refresh" aria-label="刷新验证码" data-i18n-aria="auth.cap_aria"><img id="cap" src="/api/captcha" alt="图片验证码" data-i18n-alt="auth.cap_alt"></button><span id="capq" class="capq" style="display:none"></span></div>
<p class="muted"><button type="button" id="capmode" class="linkbtn">看不清？改用文字题</button></p>
<button id="btn" style="width:100%" data-i18n="auth.signup">注册</button>
<p class="muted"><span data-i18n="auth.has_account">已有账号？</span><a href="/console/login" data-i18n="auth.signin">登录</a></p>
</div></div>
<script>
window.__pageTitleKey='auth.register_page_title';
const cap=document.getElementById('cap');
const capq=document.getElementById('capq');
let textMode=false;
async function loadTextCap(){capq.textContent=t('auth.cap_loading');try{const r=await fetch('/api/captcha?mode=text');const j=await r.json();capq.textContent=j.question||t('auth.cap_load_fail');}catch(e){capq.textContent=t('auth.cap_load_fail');}}
function refreshCap(){if(textMode){loadTextCap();}else{cap.src='/api/captcha?t='+Date.now();}}
document.getElementById('capbtn').onclick=refreshCap;
function syncCapMode(){document.getElementById('c').placeholder=textMode?t('auth.cap_text_ph'):t('auth.cap_img_ph');document.getElementById('capbtn').style.display=textMode?'none':'';capq.style.display=textMode?'':'none';document.getElementById('capmode').textContent=textMode?t('auth.cap_to_img'):t('auth.cap_to_text');}
window.__onLangChange=syncCapMode;
document.getElementById('capmode').onclick=e=>{e.preventDefault();textMode=!textMode;document.getElementById('c').value='';syncCapMode();refreshCap();};
document.getElementById('btn').onclick=async()=>{
  const btn=document.getElementById('btn'); if(btn.disabled)return; btn.disabled=true;
  try{
    const r=await fetch('/api/auth/register',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({username:u.value,password:p.value,captcha:c.value})});
    const j=await r.json();
    if(j.ok){location.href='/console';}else{document.getElementById('err').textContent=j.error||t('auth.signup_fail');refreshCap();document.getElementById('c').value='';btn.disabled=false;}
  }catch(e){document.getElementById('err').textContent=t('auth.network_err');btn.disabled=false;}
};
u.addEventListener('keydown',e=>{if(e.key==='Enter')btn.click();});
p.addEventListener('keydown',e=>{if(e.key==='Enter')btn.click();});
c.addEventListener('keydown',e=>{if(e.key==='Enter')btn.click();});
applyI18n();syncCapMode();
</script>"#,
    )
}

fn esc_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub fn oidc_login_page(code: &str, err: &str) -> String {
    let code = esc_html(code);
    let err_html = if err.is_empty() {
        String::new()
    } else {
        format!("<div class=\"err\">{}</div>", esc_html(err))
    };
    let mut body = format!(
        r#"<div class="center"><div class="langrow"><button type="button" class="langbtn ghost" data-lang="zh">中文</button><button type="button" class="langbtn ghost" data-lang="en">English</button></div><div class="card"><h1 data-i18n="auth.oidc_title">登录 RustDesk</h1>
<p class="muted" data-i18n="auth.oidc_desc">使用你的账号登录，登录后请返回 RustDesk 客户端。</p>
{err_html}
<form method="post" action="/oidc/login">
<input type="hidden" name="code" value="{code}">
<label data-i18n="auth.username">用户名</label><input name="username" autocomplete="username" required>
<label data-i18n="auth.password">密码</label><input name="password" type="password" autocomplete="current-password" required>
<label data-i18n="auth.captcha">验证码</label><div class="cap"><input name="captcha" id="c" autocomplete="off" placeholder="输入图片中的字符" data-i18n-ph="auth.cap_img_ph"><button type="button" id="capbtn" class="capbtn" title="点击刷新验证码" data-i18n-title="auth.cap_refresh" aria-label="刷新验证码" data-i18n-aria="auth.cap_aria"><img id="cap" src="/api/captcha" alt="图片验证码" data-i18n-alt="auth.cap_alt"></button><span id="capq" class="capq" style="display:none"></span></div>
<p class="muted"><button type="button" id="capmode" class="linkbtn">看不清？改用文字题</button></p>
<button style="width:100%" data-i18n="auth.signin">登录</button>
</form>
</div></div>"#
    );
    body.push_str(
        r#"<script>
window.__pageTitleKey='auth.oidc_title';
const cap=document.getElementById('cap');
const capq=document.getElementById('capq');
let textMode=false;
async function loadTextCap(){capq.textContent=t('auth.cap_loading');try{const r=await fetch('/api/captcha?mode=text');const j=await r.json();capq.textContent=j.question||t('auth.cap_load_fail');}catch(e){capq.textContent=t('auth.cap_load_fail');}}
function refreshCap(){if(textMode){loadTextCap();}else{cap.src='/api/captcha?t='+Date.now();}}
document.getElementById('capbtn').onclick=refreshCap;
function syncCapMode(){document.getElementById('c').placeholder=textMode?t('auth.cap_text_ph'):t('auth.cap_img_ph');document.getElementById('capbtn').style.display=textMode?'none':'';capq.style.display=textMode?'':'none';document.getElementById('capmode').textContent=textMode?t('auth.cap_to_img'):t('auth.cap_to_text');}
window.__onLangChange=syncCapMode;
document.getElementById('capmode').onclick=e=>{e.preventDefault();textMode=!textMode;document.getElementById('c').value='';syncCapMode();refreshCap();};
applyI18n();syncCapMode();
</script>"#,
    );
    page("登录 RustDesk", &body)
}

pub fn oidc_success_page() -> String {
    page(
        "登录成功",
        r#"<div class="center"><div class="langrow"><button type="button" class="langbtn ghost" data-lang="zh">中文</button><button type="button" class="langbtn ghost" data-lang="en">English</button></div><div class="card"><h1 class="ok" data-i18n="auth.oidc_ok_title">登录成功</h1>
<p data-i18n="auth.oidc_ok_desc">请返回 RustDesk 客户端，登录状态会自动完成。可以关闭此页面。</p>
</div></div>
<script>window.__pageTitleKey='auth.oidc_ok_title';applyI18n();</script>"#,
    )
}

/// Landing page for a relay registration link. The link itself is
/// the credential (one-time token + link key in the URL), so the
/// page never logs or echoes the token/key — it only shows the
/// link's validity and the exact command to run on the relay host
/// (built in JS from location.href).
pub fn relay_register_page_html(status: &str) -> String {
    let (key, fallback) = match status {
        "valid" => (
            "relay.regpage.valid",
            "注册链接有效。在中继机器的安装目录下运行以下命令完成注册。",
        ),
        "used" => (
            "relay.regpage.used",
            "此注册链接已被使用。请在控制台重新生成一个注册链接。",
        ),
        "expired" => (
            "relay.regpage.expired",
            "此注册链接已过期。请在控制台重新生成一个注册链接。",
        ),
        _ => (
            "relay.regpage.invalid",
            "注册链接无效。请检查链接是否完整，或在控制台重新生成。",
        ),
    };
    let cls = if status == "valid" { "ok" } else { "err" };
    let cmd_block = if status == "valid" {
        r#"<p class="muted" data-i18n="relay.regpage.cmd_hint">在中继机器上，进入 relay-agent 与 hbbr 所在目录，运行：</p>
<pre id="cmd" class="changelog"></pre>
<p><button type="button" id="copybtn" data-i18n="relay.regpage.copy">复制命令</button></p>
<p class="muted" data-i18n="relay.regpage.warn">此链接含注册密钥，相当于凭证：注册完成后即失效，请勿转发或公开。</p>
<p class="muted" data-i18n="relay.regpage.win_hint">Windows 上同样在 relay-agent.exe 所在目录运行此命令（PowerShell 或 cmd）。</p>"#
    } else {
        ""
    };
    let mut body = format!(
        r#"<div class="center"><div class="langrow"><button type="button" class="langbtn ghost" data-lang="zh">中文</button><button type="button" class="langbtn ghost" data-lang="en">English</button></div><div class="card"><h1 data-i18n="relay.regpage.title">中继节点注册</h1>
<p class="{cls}" data-i18n="{key}">{fallback}</p>
{cmd_block}
</div></div>"#
    );
    body.push_str(
        r#"<script>
window.__pageTitleKey='relay.regpage.title';
var c=document.getElementById('cmd');
if(c){c.textContent='relay-agent register --link '+location.href;}
var b=document.getElementById('copybtn');
if(b){b.onclick=function(){var t=document.getElementById('cmd').textContent;function done(){b.textContent=t('relay.regpage.copied');setTimeout(function(){applyI18n();},2000);}if(navigator.clipboard&&navigator.clipboard.writeText){navigator.clipboard.writeText(t).then(done).catch(function(){fb();});}else{fb();}function fb(){var ta=document.createElement('textarea');ta.value=t;document.body.appendChild(ta);ta.select();try{document.execCommand('copy');}catch(e){}document.body.removeChild(ta);done();}};}
applyI18n();
</script>"#,
    );
    page("中继节点注册", &body)
}

pub fn console_page() -> String {
    page("RustDesk 服务控制台", CONSOLE_BODY)
}

const CONSOLE_BODY: &str = r#"
<div class="wrap">
<div style="display:flex;justify-content:space-between;align-items:center;flex-wrap:wrap;gap:8px">
<h1 data-i18n="app.title">RustDesk 服务控制台</h1>
<div style="display:flex;gap:6px;flex-wrap:wrap;align-items:center"><span id="who" class="muted"></span><button class="langbtn ghost" data-lang="zh">中文</button><button class="langbtn ghost" data-lang="en">English</button><button class="ghost" id="pwdchg" data-i18n="pwd.title">修改密码</button><button class="ghost" id="logout" data-i18n="app.logout">退出</button></div>
</div>
<div class="card" id="pwd-card" style="display:none"><h2 data-i18n="pwd.title">修改密码</h2>
<div class="err" id="pwd-err"></div><div class="ok" id="pwd-ok"></div>
<label data-i18n="pwd.old">旧密码</label><input id="pwd-old" type="password" placeholder="当前使用的密码" data-i18n-ph="pwd.old_ph">
<label data-i18n="pwd.new">新密码</label><input id="pwd-new" type="password" placeholder="新密码（至少 6 位）" data-i18n-ph="pwd.new_ph">
<label data-i18n="pwd.confirm">确认新密码</label><input id="pwd-confirm" type="password" placeholder="再次输入新密码" data-i18n-ph="pwd.confirm_ph">
<button id="pwd-submit" data-i18n="pwd.submit">确认修改</button> <button class="ghost" id="pwd-cancel" data-i18n="app.cancel">取消</button></div>
<nav>
<button data-s="overview" class="active" data-i18n="nav.overview">概览</button>
<button data-s="devices" data-i18n="nav.devices">设备</button>
<button data-s="audit" data-i18n="nav.audit">审计日志</button>
<button data-s="users" class="adminonly" data-i18n="nav.users">用户管理</button>
<button data-s="tokens" data-i18n="nav.tokens">API Token</button>
<button data-s="genclient" data-i18n="nav.genclient">生成客户端</button>
<button data-s="settings" class="adminonly" data-i18n="nav.settings">服务器设置</button>
</nav>

<section id="s-overview" class="active">
<div class="row adminonly">
<div class="card"><div class="muted" data-i18n="ov.users">注册用户</div><div class="stat" id="stat-users">-</div></div>
<div class="card"><div class="muted" data-i18n="ov.devices">已绑定设备</div><div class="stat" id="stat-devices">-</div></div>
<div class="card"><div class="muted" data-i18n="ov.peers">已注册终端（hbbs）</div><div class="stat" id="stat-peers">-</div></div>
</div>
<div class="err" id="ov-err"></div>
<div class="card"><h2 data-i18n="ov.about">说明</h2>
<p class="muted" data-i18n="ov.about_text">客户端在「设置 → 网络」填入本服务器地址，或直接使用「生成客户端」里预绑定配置的安装包，即可配合账号登录使用。设备在客户端登录账号后会通过心跳自动绑定到所属用户。</p>
</div>
</section>

<section id="s-devices">
<div class="card"><h2 data-i18n="dev.title">设备</h2>
<p class="muted"><button class="ghost" id="dev-refresh" data-i18n="dev.refresh">刷新</button> <span id="dev-updated"></span></p>
<div id="dev-list" data-i18n="app.loading">加载中…</div></div>
<div class="card adminonly" id="relay-card"><h2 data-i18n="relay.title">中继节点</h2>
<p class="muted" data-i18n="relay.desc">中继节点由常驻代理（relay-agent）管理：代理注册后每 20 秒心跳上报状态，控制台下发的配置随心跳响应加密回传。注意：注册只建立管理关系，不会自动把节点加入中继轮询列表——轮询列表仍由服务器启动参数 RELAY_SERVERS 决定。</p>
<div class="err" id="relay-err"></div>
<label data-i18n="relay.label">备注（可选）</label><input id="relay-label" placeholder="如：香港中继" data-i18n-ph="relay.label_ph">
<label data-i18n="relay.expires">有效期（小时，1–168）</label><input id="relay-hours" type="number" value="24" min="1" max="168">
<button id="relay-gen" data-i18n="relay.gen">生成注册链接</button>
<div id="relay-new" style="margin-top:12px"></div>
<div id="relay-list" style="margin-top:12px" data-i18n="app.loading">加载中…</div>
<h2 style="margin-top:20px" data-i18n="relay.links_title">待使用的注册链接</h2>
<div id="relay-links" data-i18n="app.loading">加载中…</div></div>
<div class="card" id="relay-detail-card" style="display:none"><div id="relay-detail"></div></div>
</section>

<section id="s-audit">
<div class="card"><h2 data-i18n="aud.title">审计日志</h2>
<p class="muted" data-i18n="aud.desc">被控端上报的连接记录：谁在什么时间从哪里连到哪台设备、是否走中继、连了多久。用户名来自设备绑定，未绑定设备只显示 ID。</p>
<p><input id="aud-q" placeholder="搜索控制端/被控端 ID、用户名、主机名" data-i18n-ph="aud.search_ph" style="width:320px"> <button id="aud-search" data-i18n="aud.search">搜索</button>
<select id="aud-days" style="width:120px"><option value="1" data-i18n="aud.range_today">今天</option><option value="7" data-i18n="aud.range_7d">7 天</option><option value="30" selected data-i18n="aud.range_30d">30 天</option><option value="0" data-i18n="aud.range_all">全部</option></select>
<button class="ghost" id="aud-refresh" data-i18n="aud.refresh">刷新</button></p>
<div id="aud-list" data-i18n="app.loading">加载中…</div>
<p id="aud-pager" style="margin-top:12px"></p></div>
</section>

<section id="s-users">
<div class="card"><h2 data-i18n="usr.title">用户管理</h2>
<p><button id="usr-new-toggle" data-i18n="usr.create">新建用户</button></p>
<div id="usr-new-form" style="display:none">
<div class="err" id="usr-new-err"></div><div class="ok" id="usr-new-ok"></div>
<label data-i18n="usr.username">用户名</label><input id="usr-new-name" placeholder="用户名（3-32 位字母、数字、_-.)" data-i18n-ph="usr.new_name_ph">
<label data-i18n="usr.password">密码</label><input id="usr-new-pass" type="password" placeholder="密码（至少 6 位）" data-i18n-ph="usr.new_pass_ph">
<button id="usr-new-btn" data-i18n="usr.create_submit">创建</button> <button class="ghost" id="usr-new-cancel" data-i18n="app.cancel">取消</button>
</div>
<div id="user-list" style="margin-top:12px" data-i18n="app.loading">加载中…</div></div>
</section>

<section id="s-tokens">
<div class="card"><h2 data-i18n="tok.title">API Token（供第三方系统集成）</h2>
<p class="muted"><span data-i18n="tok.desc1">请求带</span> <code>Authorization: Bearer rdt_...</code> <span data-i18n="tok.desc2">即可调用管理接口。Token 只在创建时显示一次。</span></p>
<div class="err" id="tok-err"></div><div class="ok" id="tok-new"></div>
<label data-i18n="tok.name">名称</label><input id="tok-name" placeholder="如：内部系统" data-i18n-ph="tok.name_ph">
<button id="tok-create" data-i18n="tok.create">创建 Token</button>
<div id="tok-list" style="margin-top:12px" data-i18n="app.loading">加载中…</div></div>
</section>

<section id="s-genclient">
<div class="card"><h2 data-i18n="gen.title">生成预绑定客户端</h2>
<p class="muted" data-i18n="gen.desc">生成的安装包已内置本服务器地址与密钥（编码进文件名，界面不显示服务器与密钥设置）。把下载链接发给用户即可，用户安装后直接登录账号使用。</p>
<div class="err" id="gen-err"></div>
<label data-i18n="gen.label">备注（可选）</label><input id="gen-label" placeholder="如：公司办公电脑" data-i18n-ph="gen.label_ph">
<button id="gen-btn" data-i18n="gen.btn">生成 Windows 客户端</button>
<div id="gen-list" style="margin-top:12px" data-i18n="app.loading">加载中…</div></div>
<div class="card"><h2 data-i18n="cfg.title">官方客户端手动配置</h2>
<p class="muted" data-i18n="cfg.desc">已经装好官方 RustDesk 客户端的用户不用下载上面的安装包：在官方客户端「设置 → 网络」里填入下面四项，保存后即可登录本服务器的账号使用。下面显示的值与生成客户端内置的配置完全一致。</p>
<div id="cfg-body" data-i18n="app.loading">加载中…</div></div>
<div class="card"><h2 data-i18n="dl.title">其他平台下载</h2>
<p class="muted" data-i18n="dl.desc">其他平台请下载官方 RustDesk 客户端，安装后按上方「官方客户端手动配置」填写服务器地址与密钥，即可登录本服务器的账号使用。默认显示官方直链；勾选「镜像加速」后切换为经 GitHub 镜像下载的加速链接（iOS 为 App Store 直链，不受影响）。</p>
<label style="display:flex;gap:8px;align-items:center"><input type="checkbox" id="dl-mirror" style="width:auto;margin:0"> <span data-i18n="dl.mirror">镜像加速</span></label>
<div id="dl-body" data-i18n="app.loading">加载中…</div></div>
</section>

<section id="s-settings">
<div class="card"><h2 data-i18n="set.title">服务器设置</h2>
<div class="err" id="set-err"></div><div class="ok" id="set-ok"></div>
<label data-i18n="set.host_label">服务器公网地址（域名或 IP，不要带端口，生成客户端必填）</label>
<input id="set-host" placeholder="如 rd.example.com 或 203.0.113.10" data-i18n-ph="set.host_ph">
<label data-i18n="set.api_label">API 地址覆盖（可空，默认 http://公网地址:21114）</label>
<input id="set-api" placeholder="可空" data-i18n-ph="set.api_ph">
<label data-i18n="set.mirror_label">GitHub 加速镜像前缀（可空，下载官方客户端模板时优先使用；内置多个公共镜像自动轮询，最后直连）</label>
<input id="set-mirror" placeholder="如 https://gh-proxy.com" data-i18n-ph="set.mirror_ph">
<label style="display:flex;gap:8px;align-items:center"><input type="checkbox" id="set-hide" style="width:auto;margin:0"> <span data-i18n="set.hide">在客户端隐藏服务器/密钥设置（通过心跳下发）</span></label>
<label style="display:flex;gap:8px;align-items:center"><input type="checkbox" id="set-allowreg" style="width:auto;margin:0"> <span data-i18n="set.allowreg">允许开放注册（任何人可自助注册账号）</span></label>
<p class="muted" data-i18n="set.allowreg_note">关闭后，任何人无法自助注册；管理员在「用户管理」可建号。公网部署建议关闭。</p>
<label style="display:flex;gap:8px;align-items:center"><input type="checkbox" id="set-allowrc" style="width:auto;margin:0"> <span data-i18n="set.allowrc">允许远程修改配置（通过心跳下发到所有客户端）</span></label>
<p class="muted" data-i18n="set.allowrc_note">开启后，控制端可以操作被控端的 RustDesk 客户端界面与设置（包括改密码、改配置），不用再逐台到被控端本地勾选。仅在自有设备上开启；已在本机手动改过这一项的客户端以本机设置为准。</p>
<label style="display:flex;gap:8px;align-items:center"><input type="checkbox" id="set-mustlogin" style="width:auto;margin:0"> <span data-i18n="set.mustlogin">必须登录（所有客户端必须登录后才能连接）</span></label>
<p class="muted" data-i18n="set.mustlogin_note">开启后，未登录的客户端（没有账号登录或登录已失效）无法发起任何连接，包括直连与中继；纯 ID+Key（无账号）部署请勿开启。</p>
<button id="set-save" data-i18n="set.save">保存设置</button></div>
<div class="card"><h2 data-i18n="set.ports_title">监听端口（当前生效值）</h2>
<p class="muted" id="ports-line" data-i18n="set.ports_loading">读取中…</p>
<p class="muted" data-i18n="set.ports_desc">端口由启动参数决定，不在这里改：编辑程序目录下的 server.env（没有就从 server.env.example 复制一份），改 HBBS_PORT / HBBR_PORT / API_PORT / RELAY_SERVERS 后重启服务端。改端口后必须重新生成客户端（新客户端才会带新端口），并更新路由器端口转发；已生成的旧客户端会连不上新端口。换端口只能躲开针对默认端口的批量扫描，真正的安全仍靠服务器密钥与账号登录——知道新端口的人仍可注册 ID。</p></div>
<div class="card"><h2 data-i18n="set.svc_title">系统服务（Ubuntu）</h2>
<p class="muted" data-i18n="set.svc_desc">在程序目录执行下面命令，把服务端注册为 systemd 服务：开机自启、崩溃自动重启。服务跑的就是 start.sh，所以 server.env 里的端口与中继配置同样生效；升级时只需替换 hbbs / hbbr 两个文件，不用重装服务。</p>
<div style="font-family:monospace;background:#0d1117;border:1px solid #30363d;border-radius:6px;padding:10px">sudo ./install-service.sh<br>sudo ./uninstall-service.sh&nbsp;&nbsp;<span data-i18n="set.svc_remove"># 移除服务</span><br>systemctl status rustdesk-server-pro<br>journalctl -u rustdesk-server-pro -f&nbsp;&nbsp;<span data-i18n="set.svc_logs"># 看服务日志</span></div></div>
<div class="card"><h2 data-i18n="set.tpl_title">官方客户端模板</h2>
<p class="muted" id="tpl-status" data-i18n="set.tpl_checking">检查中…</p>
<label data-i18n="set.tpl_ver_label">版本（可空 = 最新版）</label><input id="tpl-ver" placeholder="如 1.4.2" data-i18n-ph="set.tpl_ver_ph">
<button id="tpl-btn" data-i18n="set.tpl_btn">下载/更新模板</button>
<div id="tpl-prog" style="display:none;margin-top:12px">
<div style="background:#0d1117;border:1px solid #30363d;border-radius:6px;height:14px;overflow:hidden">
<div id="tpl-bar" style="height:100%;width:0%;background:#1f6feb;transition:width .5s"></div>
</div>
<p class="muted" id="tpl-detail"></p>
</div>
<p class="muted" data-i18n="set.tpl_desc">模板是官方 RustDesk Windows 安装程序，生成客户端时会在其基础上改文件名注入配置，不修改程序本身。下载会自动在多个 GitHub 加速镜像与直连之间切换（某个源太慢会自动换下一个）。</p></div>
<div class="card"><h2 data-i18n="set.upd_title">服务端版本更新</h2>
<p class="muted" id="upd-current" data-i18n="set.upd_loading">读取中…</p>
<div class="err" id="upd-err"></div>
<button id="upd-check" data-i18n="set.upd_check">检测更新</button>
<div id="upd-result" style="display:none;margin-top:12px"></div>
<div id="upd-prog" style="display:none;margin-top:12px">
<div style="background:#0d1117;border:1px solid #30363d;border-radius:6px;height:14px;overflow:hidden">
<div id="upd-bar" style="height:100%;width:0%;background:#1f6feb;transition:width .5s"></div>
</div>
<p class="muted" id="upd-detail"></p>
</div>
<p class="muted" data-i18n="set.upd_desc">更新包来自本项目的 GitHub Releases，按当前平台自动选择。GitHub 加速用的是上方「GitHub 加速镜像前缀」设置，未配置时用内置镜像链。下载完成后新文件先暂存，点「立即重启」后由启动器（rdsvc / start.sh / start.ps1 / systemd）在重启时替换生效；直接运行 hbbs（无启动器托管）时，需手动把 hbbs.new / hbbr.new 改名覆盖原文件后再启动。</p></div>
</section>
</div>

<script>
window.__pageTitleKey='app.title';
const $=id=>document.getElementById(id);
let ME=null;
async function api(url,opts={}){const r=await fetch(url,opts);if(r.status===401){location.href='/console/login';throw new Error('unauth');}return r.json();}
async function copyText(t,btn){let ok=false;try{await navigator.clipboard.writeText(t);ok=true;}catch(e){const ta=document.createElement('textarea');ta.value=t;ta.style.position='fixed';ta.style.opacity='0';document.body.appendChild(ta);ta.select();try{ok=document.execCommand('copy');}catch(_){}ta.remove();}if(btn){const o=btn.textContent;btn.textContent=ok?window.t('app.copied'):window.t('app.copy_failed');setTimeout(()=>btn.textContent=o,1200);}}
function esc(s){return String(s==null?'':s).replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));}
// v43: responsive-table wrapper (see the CSS comment above). Returns the
// opening markup for a table: a keyboard-scrollable region with an
// accessible name, a fixed-layout table whose column headers are also
// exposed as --label-N CSS variables for the stacked-card layout, and
// a thead/tbody structure. Header entries may be '' (no label for that
// column, e.g. an actions column). Close with TABLE_CLOSE.
function cssStr(s){return String(s).replace(/\\/g,'\\\\').replace(/'/g,"\\'");}
function tableOpen(label,headers,minW,attrs,colgroup){
  let st='min-width:'+minW+'px';
  headers.forEach((h,i)=>{if(h)st+=';--label-'+(i+1)+':\''+cssStr(h)+'\'';});
  const th=headers.map(h=>'<th>'+esc(h)+'</th>').join('');
  return '<div class="table-wrapper" tabindex="0" role="region" aria-label="'+esc(label)+'"><table'+(attrs?' '+attrs:'')+' style="'+st+'">'+(colgroup||'')+'<thead><tr>'+th+'</tr></thead><tbody>';
}
const TABLE_CLOSE='</tbody></table></div>';
function loadFail(el,fnName,msg){el.innerHTML='<p class="err">'+window.t('app.load_failed')+(msg?(window.t('app.colon')+esc(msg)):'')+window.t('app.period')+'<button class="ghost" onclick="'+fnName+'()">'+window.t('app.retry')+'</button></p>';}
async function init(){
  applyI18n();
  try{ ME=await api('/api/me'); }catch(e){ if(e.message!=='unauth')$('who').textContent=t('app.load_fail_refresh'); return; }
  if(!ME.ok){location.href='/console/login';return;}
  $('who').textContent=ME.username+(ME.is_admin?t('app.who_admin'):'');
  if(!ME.is_admin){document.querySelectorAll('.adminonly').forEach(e=>e.style.display='none');}
  loadOverview(); loadDevices();
  window.addEventListener('hashchange',routeFromHash);
  routeFromHash();
}
window.__onLangChange=()=>{
  if(ME)$('who').textContent=ME.username+(ME.is_admin?t('app.who_admin'):'');
  const active=document.querySelector('nav button.active');
  const s=active?active.dataset.s:'overview';
  if(s==='overview')loadOverview(); if(s==='devices')loadDevices();
  if(s==='audit')loadAudit(AUDIT_PAGE);
  if(s==='users')loadUsers(); if(s==='tokens')loadTokens();
  if(s==='genclient'){loadGen();loadCfg();loadDl();} if(s==='settings')loadSettings();
  if(RELAY_DETAIL_ID)showRelay(RELAY_DETAIL_ID);
};
// v38: hash routing — the console section lives in the URL (#/devices,
// #/devices/relay-3), so a refresh or browser back/forward restores the
// same view instead of always landing on overview.
const SECTIONS=['overview','devices','audit','users','tokens','genclient','settings'];
function sectionAllowed(s){
  if(SECTIONS.indexOf(s)<0)return false;
  if(s==='users'||s==='settings')return !!(ME&&ME.is_admin);
  return true;
}
function parseHash(){
  const m=(location.hash||'').match(/^#\/([a-z]+)(?:\/relay-(\d+))?$/);
  if(!m||!sectionAllowed(m[1]))return {s:'overview',relay:null};
  return {s:m[1],relay:(m[1]==='devices'&&m[2])?parseInt(m[2],10):null};
}
function showSection(s,relay){
  document.querySelectorAll('nav button').forEach(x=>x.classList.remove('active'));
  const btn=document.querySelector('nav button[data-s="'+s+'"]');
  if(btn)btn.classList.add('active');
  document.querySelectorAll('section').forEach(x=>x.classList.remove('active'));
  $('s-'+s).classList.add('active');
  if(s==='overview')loadOverview();
  if(s==='devices'){
    if(relay){ loadDevices().then(()=>{ const r=parseHash(); if(r.s==='devices'&&r.relay===relay&&RELAY_DETAIL_ID!==relay)showRelay(relay); }); }
    else{ closeRelayDetail(); loadDevices(); }
  }else closeRelayDetail();
  if(s==='audit')loadAudit(AUDIT_PAGE);
  if(s==='users')loadUsers(); if(s==='tokens')loadTokens();
  if(s==='genclient'){loadGen();loadCfg();loadDl();} if(s==='settings')loadSettings();
}
function routeFromHash(){
  const r=parseHash();
  const want=r.relay?'#/devices/relay-'+r.relay:'#/'+r.s;
  if(location.hash&&location.hash!==want)history.replaceState(null,'',want);
  showSection(r.s,r.relay);
}
document.querySelectorAll('nav button').forEach(b=>b.onclick=()=>{
  const h='#/'+b.dataset.s;
  if(location.hash===h)routeFromHash(); else location.hash=h;
});
$('dev-refresh').onclick=()=>loadDevices();
$('logout').onclick=async()=>{try{await api('/api/auth/logout',{method:'POST'});}catch(e){} location.href='/console/login';};
$('pwdchg').onclick=()=>{
  const c=$('pwd-card');
  const show=c.style.display==='none';
  c.style.display=show?'':'none';
  if(show){$('pwd-old').focus();c.scrollIntoView({behavior:'smooth'});}
};
$('pwd-submit').onclick=async()=>{
  const btn=$('pwd-submit'); if(btn.disabled)return; btn.disabled=true;
  $('pwd-err').textContent='';$('pwd-ok').textContent='';
  const old=$('pwd-old').value, nw=$('pwd-new').value, cf=$('pwd-confirm').value;
  if(!old){$('pwd-err').textContent=t('pwd.empty_old');btn.disabled=false;return;}
  if(nw.length<6){$('pwd-err').textContent=t('pwd.short');btn.disabled=false;return;}
  if(nw!==cf){$('pwd-err').textContent=t('pwd.mismatch');btn.disabled=false;return;}
  try{
    const j=await api('/api/auth/change-password',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({old_password:old,new_password:nw})});
    if(j.ok){$('pwd-old').value='';$('pwd-new').value='';$('pwd-confirm').value='';$('pwd-ok').textContent=t('pwd.ok');}else{$('pwd-err').textContent=j.error||t('app.failed');}
  }catch(e){ if(e.message!=='unauth')$('pwd-err').textContent=t('app.network_err'); }
  btn.disabled=false;
};
$('pwd-cancel').onclick=()=>{
  $('pwd-old').value='';$('pwd-new').value='';$('pwd-confirm').value='';
  $('pwd-err').textContent='';$('pwd-ok').textContent='';
  $('pwd-card').style.display='none';
};

async function loadOverview(){
  if(!ME.is_admin)return;
  let j; try{ j=await api('/api/admin/overview'); }catch(e){ if(e.message!=='unauth')$('ov-err').textContent=t('ov.load_fail'); return; }
  if(!j.ok){ $('ov-err').textContent=t('ov.load_fail2')+(j.error||t('app.unknown_err')); return; }
  $('ov-err').textContent='';
  $('stat-users').textContent=j.users; $('stat-devices').textContent=j.devices; $('stat-peers').textContent=j.peers;
}
async function loadDevices(){
  let j; try{ j=await api('/api/devices'); }catch(e){ if(e.message!=='unauth')loadFail($('dev-list'),'loadDevices'); return; }
  if(!j.ok){ loadFail($('dev-list'),'loadDevices',j.error); return; }
  $('dev-updated').textContent=t('dev.updated')+new Date().toLocaleTimeString();
  if(!j.devices.length){$('dev-list').innerHTML='<p class="muted">'+t('dev.empty')+'</p>';}
  else{$('dev-list').innerHTML=tableOpen(t('dev.title'),[t('dev.id'),t('dev.status'),t('dev.hostname'),t('dev.os'),t('dev.owner'),t('dev.version'),t('dev.last_seen'),t('dev.last_relay')],820,'','<colgroup><col style="width:10%"><col style="width:8%"><col style="width:18%"><col style="width:13%"><col style="width:9%"><col style="width:7%"><col style="width:18%"><col style="width:17%"></colgroup>')+
    j.devices.map(d=>`<tr><td>${esc(d.device_id)}</td><td>${(j.now-d.last_seen)<300?'<span class="ok">'+t('dev.online')+'</span>':'<span class="muted">'+t('dev.offline')+'</span>'}</td><td class="trunc" title="${esc(d.hostname||'')}">${esc(d.hostname||'')}</td><td class="trunc" title="${esc(d.os||'')}">${esc(d.os||'')}</td><td class="trunc" title="${esc(d.username||t('dev.unbound'))}">${d.username?esc(d.username):'<span class=muted>'+t('dev.unbound')+'</span>'}</td><td>${esc(d.version||'')}</td><td class="trunc" title="${esc(new Date(d.last_seen*1000).toLocaleString())}">${new Date(d.last_seen*1000).toLocaleString()}</td><td class="trunc" title="${esc(d.last_relay||'-')}">${esc(d.last_relay||'-')}</td></tr>`).join('')+TABLE_CLOSE;}
  renderRelays(j.relays||[]);
  if(ME&&ME.is_admin)loadRelayLinks();
}
let AUDIT_PAGE=1;
function fmtDur(sec){
  if(sec==null||isNaN(sec))return '-';
  sec=Math.max(0,Math.round(sec));
  const h=Math.floor(sec/3600),m=Math.floor(sec%3600/60),s=sec%60;
  if(h)return h+'h '+m+'m';
  if(m)return m+'m '+s+'s';
  return s+'s';
}
function whoCell(id,name,username){
  const main=esc(name||id||'-');
  const sub=[id,username].filter(Boolean).map(esc).join(' · ');
  return main+(sub&&sub!==main?'<br><span class="muted">'+sub+'</span>':'');
}
async function loadAudit(page){
  AUDIT_PAGE=Math.max(1,page||1);
  const q=$('aud-q').value.trim(), days=$('aud-days').value;
  let j; try{ j=await api('/api/audit/sessions?q='+encodeURIComponent(q)+'&days='+encodeURIComponent(days)+'&page='+AUDIT_PAGE+'&per_page=20'); }catch(e){ if(e.message!=='unauth')loadFail($('aud-list'),'loadAudit'); return; }
  if(!j.ok){ loadFail($('aud-list'),'loadAudit',j.error); return; }
  if(!j.sessions.length){$('aud-list').innerHTML='<p class="muted">'+t('aud.empty')+'</p>';}
  else{$('aud-list').innerHTML=tableOpen(t('aud.title'),[t('aud.col_started'),t('aud.col_controller'),t('aud.col_controlled'),t('aud.col_ip'),t('aud.col_relay'),t('aud.col_duration'),t('aud.col_status')],780,'','<colgroup><col style="width:18%"><col style="width:19%"><col style="width:19%"><col style="width:13%"><col style="width:9%"><col style="width:9%"><col style="width:13%"></colgroup>')+
    j.sessions.map(s=>{
      const open=s.ended_at==null;
      const dur=open?fmtDur(Date.now()/1000-s.started_at):fmtDur(s.duration_sec);
      const relay=s.relayed===true?t('aud.relayed'):s.relayed===false?t('aud.direct'):t('aud.unknown');
      const closedMark=s.closed_by==='stale'?' <span class="muted">'+t('aud.closed_stale')+'</span>':s.closed_by==='offline'?' <span class="muted">'+t('aud.closed_offline')+'</span>':'';
      return `<tr><td class="trunc" title="${esc(new Date(s.started_at*1000).toLocaleString())}">${new Date(s.started_at*1000).toLocaleString()}</td><td>${whoCell(s.controller_id,s.controller_name,s.controller_username)}</td><td>${whoCell(s.device_id,s.controlled_hostname,s.controlled_username)}</td><td class="trunc" title="${esc(s.controller_ip||'-')}">${esc(s.controller_ip||'-')}</td><td>${relay}</td><td class="nowrap">${dur}</td><td>${open?'<span class="ok">'+t('aud.open')+'</span>':'<span class="muted">'+t('aud.closed')+'</span>'+closedMark}</td></tr>`;
    }).join('')+TABLE_CLOSE;}
  const pages=Math.max(1,Math.ceil(j.total/j.per_page));
  $('aud-pager').innerHTML='<button class="ghost" id="aud-prev"'+(AUDIT_PAGE<=1?' disabled':'')+'>'+t('aud.prev')+'</button> <span class="muted">'+t('aud.page').replace('{p}',AUDIT_PAGE).replace('{n}',pages).replace('{t}',j.total)+'</span> <button class="ghost" id="aud-next"'+(AUDIT_PAGE>=pages?' disabled':'')+'>'+t('aud.next')+'</button>';
  $('aud-prev').onclick=()=>loadAudit(AUDIT_PAGE-1);
  $('aud-next').onclick=()=>loadAudit(AUDIT_PAGE+1);
}
$('aud-search').onclick=()=>loadAudit(1);
$('aud-days').onchange=()=>loadAudit(1);
$('aud-refresh').onclick=()=>loadAudit(AUDIT_PAGE);
$('aud-q').addEventListener('keydown',e=>{if(e.key==='Enter')loadAudit(1);});
let RELAYS=[];
function renderRelays(relays){
  if(!ME||!ME.is_admin)return;
  RELAYS=relays;
  const el=$('relay-list'); if(!el)return;
  if(!relays.length){el.innerHTML='<p class="muted">'+t('relay.empty')+'</p>';return;}
  el.innerHTML=tableOpen(t('relay.title'),[t('relay.node'),t('relay.status'),t('relay.address'),t('relay.port'),t('relay.sessions'),t('relay.throughput'),t('relay.last_hb')],760,'','<colgroup><col style="width:17%"><col style="width:10%"><col style="width:22%"><col style="width:9%"><col style="width:10%"><col style="width:13%"><col style="width:19%"></colgroup>')+
    relays.map(r=>`<tr><td><a href="javascript:void(0)" onclick="showRelay(${r.id})">${esc(r.name)}</a><span class="badge">${t('relay.badge')}</span></td><td>${r.online?'<span class="ok">'+t('relay.online')+'</span>':'<span class="muted">'+t('relay.offline')+'</span>'}</td><td class="trunc" title="${esc(r.address||r.last_ip||'')}">${esc(r.address||r.last_ip||'')}</td><td>${r.listen_port||'-'}</td><td>${r.sessions}</td><td>${r.total_kbps} kb/s</td><td class="trunc" title="${r.last_heartbeat?esc(new Date(r.last_heartbeat*1000).toLocaleString()):t('relay.never')}">${r.last_heartbeat?new Date(r.last_heartbeat*1000).toLocaleString():t('relay.never')}</td></tr>`).join('')+TABLE_CLOSE;
}
async function loadRelayLinks(){
  let j; try{ j=await api('/api/admin/relays/reg-tokens'); }catch(e){ if(e.message!=='unauth')$('relay-links').innerHTML='<p class="err">'+t('relay.load_fail')+'</p>'; return; }
  if(!j.ok){ $('relay-links').innerHTML='<p class="err">'+esc(j.error||t('relay.load_fail'))+'</p>'; return; }
  const open=j.tokens.filter(x=>!x.used_at);
  if(!open.length){$('relay-links').innerHTML='<p class="muted">'+t('relay.links_empty')+'</p>';return;}
  $('relay-links').innerHTML=tableOpen(t('relay.links_title'),[t('relay.label'),t('relay.links_created'),t('relay.links_expires'),''],560,'','<colgroup><col style="width:30%"><col style="width:25%"><col style="width:25%"><col style="width:20%"></colgroup>')+
    open.map(x=>`<tr><td class="trunc" title="${esc(x.label||x.token_id.slice(0,8))}">${esc(x.label||x.token_id.slice(0,8))}</td><td class="trunc" title="${esc(new Date(x.created_at*1000).toLocaleString())}">${new Date(x.created_at*1000).toLocaleString()}</td><td class="trunc" title="${esc(new Date(x.expires_at*1000).toLocaleString())}">${new Date(x.expires_at*1000).toLocaleString()}</td><td><button class="danger" onclick="relayRevokeLink('${esc(x.token_id)}')">${t('relay.links_revoke')}</button></td></tr>`).join('')+TABLE_CLOSE;
}
async function relayRevokeLink(tokenId){
  if(!confirm(t('relay.links_revoke_confirm')))return;
  const j=await api('/api/admin/relays/reg-tokens/'+encodeURIComponent(tokenId),{method:'DELETE'});
  if(j.ok)loadRelayLinks(); else alert(j.error||t('app.op_failed'));
}
$('relay-gen').onclick=async()=>{
  const btn=$('relay-gen'); if(btn.disabled)return; btn.disabled=true;
  try{
    const hours=parseInt($('relay-hours').value,10)||24;
    const j=await api('/api/admin/relays/reg-token',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({label:$('relay-label').value,expires_hours:Math.max(1,Math.min(168,hours))})});
    if(j.ok){
      $('relay-err').textContent='';
      const cmd='relay-agent register --link '+j.link;
      $('relay-new').innerHTML='<p class="ok">'+t('relay.link_once')+'</p><div class="dl" style="margin-top:6px"><input readonly value="'+esc(j.link)+'"><button class="ghost" onclick="copyText(this.closest(\'.dl\').querySelector(\'input\').value,this)">'+t('app.copy')+'</button></div><p class="muted">'+t('relay.link_expires')+new Date(j.expires_at*1000).toLocaleString()+'</p><p class="muted">'+t('relay.cmd_label')+'</p><div class="dl"><input readonly value="'+esc(cmd)+'"><button class="ghost" onclick="copyText(this.closest(\'.dl\').querySelector(\'input\').value,this)">'+t('app.copy')+'</button></div>';
      loadRelayLinks();
    }else{$('relay-err').textContent=j.error||t('relay.gen_fail');}
  }catch(e){ if(e.message!=='unauth')$('relay-err').textContent=t('app.network_err'); }
  btn.disabled=false;
};
let RELAY_DETAIL_ID=null;
let RELAY_DETAIL_NAME='';
async function showRelay(id){
  let j; try{ j=await api('/api/admin/relays/'+id); }catch(e){ if(e.message!=='unauth')alert(t('relay.load_fail')); return; }
  if(!j.ok){ alert(j.error||t('relay.load_fail')); if(location.hash==='#/devices/relay-'+id)location.hash='#/devices'; return; }
  const active=document.querySelector('nav button.active');
  if(!active||active.dataset.s!=='devices')return;
  RELAY_DETAIL_ID=id;
  const h='#/devices/relay-'+id;
  if(location.hash!==h)location.hash=h;
  RELAY_DETAIL_NAME=j.node.name||'';
  const n=j.node, cfg=n.config||{};
  const lim=v=>(v==null?'':v);
  $('relay-detail').innerHTML='<p><button class="ghost" onclick="backRelay()">'+t('relay.detail_back')+'</button></p>'+
    '<h2>'+esc(n.name)+'<span class="badge">'+t('relay.badge')+'</span></h2>'+
    '<div class="table-wrapper" tabindex="0" role="region" aria-label="'+esc(n.name||t('relay.title'))+'"><table class="kv" style="min-width:360px"><colgroup><col style="width:30%"><col style="width:70%"></colgroup><tbody>'+
    '<tr><th>'+t('relay.status')+'</th><td>'+(n.online?'<span class="ok">'+t('relay.online')+'</span>':'<span class="muted">'+t('relay.offline')+'</span>')+'</td></tr>'+
    '<tr><th>'+t('relay.address')+'</th><td>'+esc(n.address||t('relay.unknown_time'))+'</td></tr>'+
    '<tr><th>'+t('relay.last_ip')+'</th><td>'+esc(n.last_ip||t('relay.unknown_time'))+'</td></tr>'+
    '<tr><th>'+t('relay.agent_ver')+'</th><td>'+esc(n.agent_version||t('relay.unknown_time'))+'</td></tr>'+
    '<tr><th>'+t('relay.hbbr_ver')+'</th><td>'+esc(n.hbbr_version||t('relay.unknown_time'))+'</td></tr>'+
    '<tr><th>'+t('relay.hbbr_running')+'</th><td>'+(n.hbbr_running?'<span class="ok">'+t('relay.hbbr_running')+'</span>':'<span class="muted">'+t('relay.hbbr_down')+'</span>')+'</td></tr>'+
    '<tr><th>'+t('relay.started_at')+'</th><td>'+(n.hbbr_started_at?new Date(n.hbbr_started_at*1000).toLocaleString():t('relay.unknown_time'))+'</td></tr>'+
    '<tr><th>'+t('relay.last_hb')+'</th><td>'+(n.last_heartbeat?new Date(n.last_heartbeat*1000).toLocaleString():t('relay.never'))+'</td></tr>'+
    '<tr><th>'+t('relay.sessions')+'</th><td>'+n.sessions+'</td></tr>'+
    '<tr><th>'+t('relay.throughput')+'</th><td>'+n.total_kbps+' kb/s</td></tr>'+
    '<tr><th>'+t('relay.key_done')+'</th><td>'+(n.key_delivered?'<span class="ok">'+t('relay.key_done')+'</span>':'<span class="muted">'+t('relay.key_pending')+'</span>')+'</td></tr>'+
    '<tr><th>'+t('relay.cfg_ver')+'</th><td>'+n.applied_version+' / '+n.config_version+'</td></tr>'+
    '<tr><th>'+t('relay.pending_cmd')+'</th><td>'+esc(n.pending_command||'-')+'</td></tr></tbody></table></div>'+
    '<h2 style="margin-top:20px">'+t('relay.cfg_title')+'</h2>'+
    '<p class="muted">'+t('relay.port_change_note')+'</p><div class="err" id="relay-cfg-err"></div><div class="ok" id="relay-cfg-ok"></div>'+
    '<label>'+t('relay.port')+'</label><input id="rcfg-port" type="number" value="'+(cfg.port||21117)+'">'+
    '<label>'+t('relay.limit_speed')+'</label><input id="rcfg-ls" type="number" step="any" min="0" value="'+lim(cfg.limit_speed)+'" placeholder="'+t('relay.default_hint')+'">'+
    '<label>'+t('relay.limit_total')+'</label><input id="rcfg-tb" type="number" step="any" min="0" value="'+lim(cfg.total_bandwidth)+'" placeholder="'+t('relay.default_hint')+'">'+
    '<label>'+t('relay.limit_single')+'</label><input id="rcfg-sb" type="number" step="any" min="0" value="'+lim(cfg.single_bandwidth)+'" placeholder="'+t('relay.default_hint')+'">'+
    '<p><button id="rcfg-push" onclick="relayPushCfg('+n.id+')">'+t('relay.push')+'</button> '+
    '<button class="ghost" onclick="relayRestart('+n.id+')">'+t('relay.restart')+'</button> '+
    '<button class="danger" onclick="relayDelete('+n.id+')">'+t('relay.delete')+'</button></p>';
  $('relay-detail-card').style.display='';
  $('relay-detail-card').scrollIntoView({behavior:'smooth'});
}
function closeRelayDetail(){ RELAY_DETAIL_ID=null; RELAY_DETAIL_NAME=''; $('relay-detail-card').style.display='none'; }
function backRelay(){ closeRelayDetail(); if(location.hash!== '#/devices')location.hash='#/devices'; }
async function relayPushCfg(id){
  const btn=$('rcfg-push'); if(btn.disabled)return; btn.disabled=true;
  const num=v=>{const s=String(v).trim(); if(!s)return null; const n=parseFloat(s); return isNaN(n)?null:n;};
  try{
    const j=await api('/api/admin/relays/'+id+'/config',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({port:parseInt($('rcfg-port').value,10),limit_speed:num($('rcfg-ls').value),total_bandwidth:num($('rcfg-tb').value),single_bandwidth:num($('rcfg-sb').value)})});
    if(j.ok){$('relay-cfg-err').textContent='';$('relay-cfg-ok').textContent=t('relay.push_ok');showRelay(id);}else{$('relay-cfg-ok').textContent='';$('relay-cfg-err').textContent=j.error||t('relay.push_fail');}
  }catch(e){ if(e.message!=='unauth')$('relay-cfg-err').textContent=t('app.network_err'); }
  btn.disabled=false;
}
async function relayRestart(id){
  if(!confirm(t('relay.restart_confirm')))return;
  const j=await api('/api/admin/relays/'+id+'/restart',{method:'POST'});
  if(j.ok){alert(t('relay.restart_ok'));showRelay(id);}else alert(j.error||t('app.op_failed'));
}
async function relayDelete(id){
  if(!confirm(t('relay.delete_confirm').replace('{n}',RELAY_DETAIL_NAME)))return;
  const j=await api('/api/admin/relays/'+id,{method:'DELETE'});
  if(j.ok){backRelay();loadDevices();}else alert(j.error||t('app.op_failed'));
}
let USERS=[];
async function loadUsers(){
  let j; try{ j=await api('/api/admin/users'); }catch(e){ if(e.message!=='unauth')loadFail($('user-list'),'loadUsers'); return; }
  if(!j.ok){ loadFail($('user-list'),'loadUsers',j.error); return; }
  USERS=j.users;
  $('user-list').innerHTML=tableOpen(t('usr.title'),[t('usr.username'),t('usr.role'),t('usr.status'),t('usr.created'),t('usr.actions')],720,'','<colgroup><col style="width:18%"><col style="width:10%"><col style="width:10%"><col style="width:20%"><col style="width:42%"></colgroup>')+
    j.users.map((u,i)=>`<tr><td class="trunc" title="${esc(u.username)}">${esc(u.username)}</td><td>${u.is_admin?t('usr.admin'):t('usr.regular')}</td><td>${u.disabled?t('usr.disabled'):t('usr.active')}</td><td class="trunc" title="${esc(new Date(u.created_at*1000).toLocaleString())}">${new Date(u.created_at*1000).toLocaleString()}</td>
    <td><button class="ghost" onclick="uact(USERS[${i}].username,'${u.disabled?'enable':'disable'}')">${u.disabled?t('usr.enable'):t('usr.disable')}</button>
    <button class="ghost" onclick="uact(USERS[${i}].username,'${u.is_admin?'unadmin':'admin'}')">${u.is_admin?t('usr.unadmin'):t('usr.make_admin')}</button>
    <button class="ghost" onclick="upwd(USERS[${i}].username)">${t('usr.reset_pwd')}</button>
    <button class="danger" onclick="uact(USERS[${i}].username,'delete')">${t('app.delete')}</button></td></tr>`).join('')+TABLE_CLOSE;
}
async function uact(name,act){
  if(act==='delete'&&!confirm(t('usr.delete_confirm').replace('{n}',name)))return;
  const j=await api('/api/admin/users/'+encodeURIComponent(name)+'/'+act,{method:'POST'}); if(j.ok)loadUsers(); else alert(j.error||t('app.op_failed'));
}
async function upwd(name){
  const pw=prompt(t('usr.pwd_prompt').replace('{n}',name)); if(!pw)return;
  const j=await api('/api/admin/users/'+encodeURIComponent(name)+'/password',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({password:pw})});
  alert(j.ok?t('usr.pwd_reset'):(j.error||t('app.failed')));
}
$('usr-new-toggle').onclick=()=>{
  const f=$('usr-new-form');
  const show=f.style.display==='none';
  f.style.display=show?'':'none';
  if(show)$('usr-new-name').focus();
};
$('usr-new-cancel').onclick=()=>{
  $('usr-new-name').value='';$('usr-new-pass').value='';
  $('usr-new-err').textContent='';$('usr-new-ok').textContent='';
  $('usr-new-form').style.display='none';
};
$('usr-new-btn').onclick=async()=>{
  const btn=$('usr-new-btn'); if(btn.disabled)return; btn.disabled=true;
  $('usr-new-err').textContent='';$('usr-new-ok').textContent='';
  try{
    const j=await api('/api/admin/users',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({username:$('usr-new-name').value,password:$('usr-new-pass').value})});
    if(j.ok){$('usr-new-name').value='';$('usr-new-pass').value='';$('usr-new-ok').textContent=t('usr.created_ok');loadUsers();}else{$('usr-new-err').textContent=j.error||t('app.failed');}
  }catch(e){ if(e.message!=='unauth')$('usr-new-err').textContent=t('app.network_err'); }
  btn.disabled=false;
};
let TOKENS=[];
async function loadTokens(){
  let j; try{ j=await api('/api/tokens'); }catch(e){ if(e.message!=='unauth')loadFail($('tok-list'),'loadTokens'); return; }
  if(!j.ok){ loadFail($('tok-list'),'loadTokens',j.error); return; }
  TOKENS=j.tokens;
  $('tok-list').innerHTML=j.tokens.length?tableOpen(t('tok.title'),[t('tok.name'),t('tok.created'),t('tok.last_used'),''],560,'','<colgroup><col style="width:30%"><col style="width:25%"><col style="width:25%"><col style="width:20%"></colgroup>')+
    j.tokens.map((t,i)=>`<tr><td class="trunc" title="${esc(t.name)}">${esc(t.name)}</td><td class="trunc" title="${esc(new Date(t.created_at*1000).toLocaleString())}">${new Date(t.created_at*1000).toLocaleString()}</td><td class="trunc" title="${t.last_used_at?esc(new Date(t.last_used_at*1000).toLocaleString()):'-'}">${t.last_used_at?new Date(t.last_used_at*1000).toLocaleString():'-'}</td><td><button class="danger" onclick="tdel(${i})">${window.t('tok.revoke')}</button></td></tr>`).join('')+TABLE_CLOSE:'<p class="muted">'+t('tok.empty')+'</p>';
}
$('tok-create').onclick=async()=>{
  const btn=$('tok-create'); if(btn.disabled)return; btn.disabled=true;
  try{
    const j=await api('/api/tokens',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:$('tok-name').value})});
    if(j.ok){$('tok-err').textContent='';$('tok-new').innerHTML=t('tok.new_label')+'<div class="dl" style="margin-top:6px"><input readonly id="tok-new-val" value="'+esc(j.token)+'"><button class="ghost" onclick="copyText(document.getElementById(\'tok-new-val\').value,this)">'+t('app.copy')+'</button></div>'; loadTokens();}else{$('tok-err').textContent=j.error||t('app.failed');}
  }catch(e){ if(e.message!=='unauth')$('tok-err').textContent=t('app.network_err'); }
  btn.disabled=false;
};
async function tdel(i){
  const t=TOKENS[i]; if(!t)return;
  if(!confirm(window.t('tok.revoke_confirm').replace('{n}',t.name)))return;
  const j=await api('/api/tokens/'+t.id,{method:'DELETE'}); if(j.ok)loadTokens(); else alert(j.error||window.t('tok.revoke_failed'));
}
let GEN=[];
async function loadGen(){
  let j; try{ j=await api('/api/admin/gen-clients'); }catch(e){ if(e.message!=='unauth')loadFail($('gen-list'),'loadGen'); return; }
  if(!j.ok){ loadFail($('gen-list'),'loadGen',j.error); return; }
  GEN=j.clients;
  $('gen-list').innerHTML=j.clients.length?tableOpen(t('gen.title'),[t('gen.file'),t('gen.remark'),t('gen.by'),t('gen.time'),t('gen.link'),''],760,'class="gent"','<colgroup><col style="width:21%"><col style="width:9%"><col style="width:9%"><col style="width:17%"><col style="width:28%"><col style="width:16%"></colgroup>')+
    j.clients.map((c,i)=>`<tr><td class="fname">${esc(c.filename)}</td><td class="trunc" title="${esc(c.label||'')}">${esc(c.label||'')}</td><td class="trunc" title="${esc(c.created_by||'')}">${esc(c.created_by||'')}</td><td class="trunc" title="${esc(new Date(c.created_at*1000).toLocaleString())}">${new Date(c.created_at*1000).toLocaleString()}</td>
    <td><div class="dl"><input readonly value="${esc(c.download_url)}"><button class="ghost" onclick="copyText(this.closest('.dl').querySelector('input').value,this)">${t('app.copy')}</button><button class="ghost" onclick="window.open(this.closest('.dl').querySelector('input').value,'_blank')">${t('app.download')}</button></div></td>
    <td class="nowrap"><button class="danger" onclick="gdel(${i})">${t('app.delete')}</button></td></tr>`).join('')+TABLE_CLOSE:'<p class="muted">'+(ME.is_admin?t('gen.empty_admin'):t('gen.empty_user'))+'</p>';
}
async function loadCfg(){
  let j; try{ j=await api('/api/client-config'); }catch(e){ if(e.message!=='unauth')loadFail($('cfg-body'),'loadCfg'); return; }
  if(!j.ok){ loadFail($('cfg-body'),'loadCfg',j.error); return; }
  if(!j.configured){ $('cfg-body').innerHTML='<p class="muted">'+(ME.is_admin?t('cfg.unconfigured_admin'):t('cfg.unconfigured_user'))+'</p>'; return; }
  const row=(label,val)=>'<label>'+label+'</label><div class="dl"><input readonly value="'+esc(val)+'"><button class="ghost" onclick="copyText(this.closest(\'.dl\').querySelector(\'input\').value,this)">'+t('app.copy')+'</button></div>';
  $('cfg-body').innerHTML=row(t('cfg.host'),j.host)+row(t('cfg.relay'),j.relay)+row(t('cfg.api'),j.api)+row(t('cfg.key'),j.key)+'<div class="dl" style="margin-top:8px"><input type="hidden" value="'+esc(j.share)+'"><button onclick="copyText(this.closest(\'.dl\').querySelector(\'input\').value,this)">'+t('cfg.copy_all')+'</button></div><p class="muted">'+t('cfg.copy_hint')+'</p>';
}
let DL_DATA=null;
let DL_MIRROR=false;
try{DL_MIRROR=localStorage.getItem('rdp-dl-mirror')==='1';}catch(e){}
function renderDl(){
  if(!DL_DATA)return;
  const j=DL_DATA;
  if(!j.platforms.length){$('dl-body').innerHTML='<p class="muted">'+t('dl.empty')+'</p>';return;}
  const row=p=>{
    let href=p.url, label=t('dl.official');
    if(p.id==='ios'){label=t('dl.appstore');}
    else if(DL_MIRROR&&p.mirrors&&p.mirrors.length){href=p.mirrors[0];label=t('dl.mirror_link');}
    return '<tr><td>'+esc(p.name)+'</td><td class="fname">'+esc(p.file||'App Store')+'</td><td class="nowrap"><a href="'+esc(href)+'" target="_blank" rel="noopener">'+label+'</a></td></tr>';
  };
  $('dl-body').innerHTML='<p class="muted">'+t('dl.latest')+esc(j.version)+'</p>'+tableOpen(t('dl.title'),[t('dl.platform'),t('dl.file'),t('dl.download')],520,'','<colgroup><col style="width:20%"><col style="width:55%"><col style="width:25%"></colgroup>')+j.platforms.map(row).join('')+TABLE_CLOSE;
}
async function loadDl(){
  let j; try{ j=await api('/api/client-downloads'); }catch(e){ if(e.message!=='unauth')loadFail($('dl-body'),'loadDl'); return; }
  if(!j.ok){ loadFail($('dl-body'),'loadDl',j.error); return; }
  DL_DATA=j; renderDl();
}
$('dl-mirror').checked=DL_MIRROR;
$('dl-mirror').onchange=e=>{DL_MIRROR=e.target.checked;try{localStorage.setItem('rdp-dl-mirror',DL_MIRROR?'1':'0');}catch(_){}renderDl();};
$('gen-btn').onclick=async()=>{
  const btn=$('gen-btn'); if(btn.disabled)return; btn.disabled=true;
  try{
    const j=await api('/api/admin/gen-client',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({label:$('gen-label').value})});
    if(j.ok){$('gen-err').textContent='';loadGen();}else{$('gen-err').textContent=j.error||t('gen.failed');}
  }catch(e){ if(e.message!=='unauth')$('gen-err').textContent=t('app.network_err'); }
  btn.disabled=false;
};
async function gdel(i){
  const c=GEN[i]; if(!c)return;
  if(!confirm(t('gen.delete_confirm').replace('{n}',(c.label||c.filename))))return;
  const j=await api('/api/admin/gen-clients/'+c.id,{method:'DELETE'}); if(j.ok)loadGen(); else alert(j.error||t('gen.delete_failed'));
}
async function loadSettings(){
  let j; try{ j=await api('/api/admin/settings'); }catch(e){ if(e.message!=='unauth')$('set-err').textContent=t('set.load_fail'); return; }
  if(!j.ok){ $('set-err').textContent=t('set.load_fail2')+(j.error||t('app.unknown_err')); return; }
  $('set-host').value=j.settings.public_host||''; $('set-api').value=j.settings.api_url_override||'';
  $('set-mirror').value=j.settings.github_mirror||'';
  $('set-hide').checked=!!j.settings.hide_server_settings;
  $('set-allowreg').checked=j.settings.allow_register!==false;
  $('set-allowrc').checked=!!j.settings.allow_remote_config;
  $('set-mustlogin').checked=!!j.settings.must_login;
  if(j.ports){let pl=t('set.ports_rendezvous')+j.ports.rendezvous+t('set.ports_nat')+j.ports.nat_test+t('set.ports_ws')+j.ports.ws_rendezvous+t('set.ports_relay')+j.ports.relay+t('set.ports_ws')+j.ports.ws_relay+t('set.ports_api')+j.ports.api; if(j.ports.console){pl+=t('set.ports_console')+j.ports.console_listen;}else{pl+=t('set.ports_same');} if(j.ports.tls){pl+=t('set.ports_tls');} $('ports-line').textContent=pl;}
  $('tpl-status').textContent=j.template_ready?(t('set.tpl_ready')+(j.template_version||'')):t('set.tpl_not_dl');
  pollTpl();
  loadUpdate();
}
$('set-save').onclick=async()=>{
  const btn=$('set-save'); if(btn.disabled)return; btn.disabled=true;
  $('set-ok').textContent=''; $('set-err').textContent='';
  try{
    const j=await api('/api/admin/settings',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({public_host:$('set-host').value,api_url_override:$('set-api').value,github_mirror:$('set-mirror').value,hide_server_settings:$('set-hide').checked,allow_register:$('set-allowreg').checked,allow_remote_config:$('set-allowrc').checked,must_login:$('set-mustlogin').checked})});
    if(j.ok){$('set-ok').textContent=t('set.saved');}else{$('set-err').textContent=j.error||t('app.failed');}
  }catch(e){ if(e.message!=='unauth')$('set-err').textContent=t('app.network_err'); }
  btn.disabled=false;
};
let tplTimer=null, tplLastBytes=0, tplLastTime=0, tplSpeed=0;
function fmtMB(n){return (n/1048576).toFixed(2)+' MB';}
function fmtSpeed(bps){return bps>=1048576?(bps/1048576).toFixed(2)+' MB/s':(bps/1024).toFixed(0)+' KB/s';}
async function pollTpl(){
  let j; try{ j=await api('/api/admin/template/status'); }catch(e){ return; }
  if(!j.ok)return;
  const now=Date.now();
  if(tplLastTime>0 && now>tplLastTime && j.downloaded>=tplLastBytes){
    const inst=(j.downloaded-tplLastBytes)/((now-tplLastTime)/1000);
    tplSpeed=tplSpeed>0?tplSpeed*0.6+inst*0.4:inst;
  }
  tplLastBytes=j.downloaded; tplLastTime=now;
  if(j.state==='downloading'||j.running){
    $('tpl-prog').style.display='block'; $('tpl-btn').disabled=true;
    const pct=j.total>0?Math.min(100,j.downloaded/j.total*100):0;
    $('tpl-bar').style.width=pct+'%';
    $('tpl-status').textContent=t('set.tpl_downloading')+(j.version?(' '+j.version):'');
    $('tpl-detail').textContent=(j.total>0?(fmtMB(j.downloaded)+' / '+fmtMB(j.total)+t('app.po')+pct.toFixed(1)+'%'+t('app.pc')):fmtMB(j.downloaded))+
      t('set.tpl_speed')+fmtSpeed(tplSpeed)+(j.source?(t('set.tpl_source')+j.source):'');
    tplTimer=setTimeout(pollTpl,1000);
  }else{
    $('tpl-btn').disabled=false;
    if(j.state==='done'){
      $('tpl-bar').style.width='100%';
      $('tpl-detail').textContent=t('set.tpl_done');
      $('tpl-status').textContent=t('set.tpl_ready')+(j.template_version||j.version||'');
    }else if(j.state==='up-to-date'){
      $('tpl-prog').style.display='none';
      $('tpl-detail').textContent='';
      $('tpl-status').textContent=t('set.tpl_uptodate').replace('{v}',j.version||j.template_version||'');
    }else if(j.state==='error'){
      $('tpl-status').textContent=t('set.tpl_failed')+(j.error||t('app.unknown_err'));
      $('tpl-detail').textContent='';
    }else if(j.template_ready){
      $('tpl-status').textContent=t('set.tpl_ready')+(j.template_version||'');
    }
    if(tplTimer){clearTimeout(tplTimer);tplTimer=null;}
  }
}
$('tpl-btn').onclick=async()=>{
  tplLastBytes=0; tplLastTime=0; tplSpeed=0;
  const j=await api('/api/admin/template/download',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({version:$('tpl-ver').value})});
  if(!j.ok){$('tpl-status').textContent=t('set.tpl_start_failed')+(j.error||'');return;}
  $('tpl-prog').style.display='block'; $('tpl-status').textContent=t('set.tpl_downloading2');
  pollTpl();
};
let UPD={current:'',latest:null};
async function loadUpdate(){
  let j; try{ j=await api('/api/admin/update/status'); }catch(e){ if(e.message!=='unauth')$('upd-current').textContent=t('set.upd_load_fail'); return; }
  if(!j.ok){ $('upd-current').textContent=t('set.upd_load_fail2')+(j.error||t('app.unknown_err')); return; }
  UPD.current=j.current||'';
  $('upd-current').textContent=t('set.upd_current')+j.current+t('set.upd_base')+j.upstream_base+t('set.upd_platform')+j.platform+(j.staged?(t('set.upd_staged')+j.staged_version+t('set.upd_staged2')):'');
  if(j.staged){ showStaged(j.staged_version); }
  if(j.running){ pollUpd(); }
}
$('upd-check').onclick=async()=>{
  const btn=$('upd-check'); if(btn.disabled)return; btn.disabled=true;
  $('upd-err').textContent=''; $('upd-result').style.display='none'; $('upd-result').innerHTML='';
  try{
    const j=await api('/api/admin/update/check',{method:'POST'});
    if(!j.ok){ $('upd-err').textContent=j.error||t('set.upd_check_fail'); return; }
    $('upd-result').style.display='block';
    if(!j.has_update){
      $('upd-result').innerHTML='<p class="ok">'+t('set.upd_latest')+esc(j.latest)+t('set.upd_latest2')+'</p>';
      return;
    }
    UPD.latest=j.latest;
    let notesHtml='';
    if(j.notes_unavailable){ notesHtml='<p class="muted">'+t('set.upd_notes_na')+'</p>'; }
    else if(!j.release_notes){ notesHtml='<p class="muted">'+t('set.upd_no_notes')+'</p>'; }
    else { notesHtml='<div class="changelog" id="upd-notes"></div>'; }
    $('upd-result').innerHTML='<p>'+t('set.upd_found')+'<b>'+esc(j.latest)+'</b>'+(j.published_at?(t('set.upd_released')+esc(j.published_at.slice(0,10))+t('app.pc')):'')+'</p>'+notesHtml+'<p><button id="upd-dl">'+t('set.upd_to')+esc(j.latest)+'</button></p>';
    const notesEl=$('upd-notes'); if(notesEl) notesEl.textContent=j.release_notes;
    $('upd-dl').onclick=()=>{
      if(!confirm(t('set.upd_confirm').replace('{v}',UPD.latest)))return;
      startUpdDownload(UPD.latest);
    };
  }catch(e){ if(e.message!=='unauth')$('upd-err').textContent=t('set.upd_check_netfail'); }
  finally{ btn.disabled=false; }
};
async function startUpdDownload(version){
  const j=await api('/api/admin/update/download',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({version})});
  if(!j.ok){$('upd-err').textContent=j.error||t('set.upd_start_failed');return;}
  $('upd-prog').style.display='block';
  updLastBytes=0; updLastTime=0; updSpeed=0;
  pollUpd();
}
let updTimer=null, updLastBytes=0, updLastTime=0, updSpeed=0;
async function pollUpd(){
  let j; try{ j=await api('/api/admin/update/status'); }catch(e){ return; }
  if(!j.ok)return;
  const now=Date.now();
  if(updLastTime>0 && now>updLastTime && j.downloaded>=updLastBytes){
    const inst=(j.downloaded-updLastBytes)/((now-updLastTime)/1000);
    updSpeed=updSpeed>0?updSpeed*0.6+inst*0.4:inst;
  }
  updLastBytes=j.downloaded; updLastTime=now;
  if(j.state==='downloading'||j.state==='extracting'||j.running){
    $('upd-prog').style.display='block';
    const pct=j.total>0?Math.min(100,j.downloaded/j.total*100):0;
    $('upd-bar').style.width=pct+'%';
    $('upd-detail').textContent=j.state==='extracting'?t('set.upd_extracting'):(j.total>0?(fmtMB(j.downloaded)+' / '+fmtMB(j.total)+t('app.po')+pct.toFixed(1)+'%'+t('app.pc')):fmtMB(j.downloaded))+t('set.tpl_speed')+fmtSpeed(updSpeed)+(j.source?(t('set.tpl_source')+j.source):'');
    updTimer=setTimeout(pollUpd,1000);
  }else{
    if(updTimer){clearTimeout(updTimer);updTimer=null;}
    if(j.state==='done'){
      $('upd-bar').style.width='100%';
      $('upd-detail').textContent=t('set.tpl_done');
      showStaged(j.version);
      loadUpdate();
    }else if(j.state==='error'){
      $('upd-err').textContent=t('set.upd_failed')+(j.error||t('app.unknown_err'));
      $('upd-detail').textContent='';
    }
  }
}
function showStaged(version){
  $('upd-result').style.display='block';
  $('upd-result').innerHTML='<p class="ok">'+t('set.upd_staged_msg').replace('{v}',esc(version))+'</p><p><button id="upd-restart">'+t('set.upd_restart')+'</button></p><p class="muted">'+t('set.upd_restart_note')+'</p>';
  $('upd-restart').onclick=async()=>{
    if(!confirm(t('set.upd_restart_confirm').replace('{v}',version)))return;
    const j=await api('/api/admin/update/restart',{method:'POST'});
    if(!j.ok){$('upd-err').textContent=j.error||t('set.upd_restart_failed');return;}
    sessionStorage.setItem('upd-just-restarted','1');
    $('upd-result').innerHTML='<p class="muted">'+t('set.upd_restarting')+'</p>';
    setTimeout(()=>waitForServer(0,UPD.current),4000);
  };
}
async function waitForServer(n,oldVer){
  if(n>45){ $('upd-result').innerHTML='<p class="err">'+t('set.upd_timeout')+'</p>'; return; }
  try{
    const r=await fetch('/api/admin/update/status');
    if(r.ok){ const j=await r.json(); if(j.ok&&j.current&&j.current!==oldVer){ location.reload(); return; } }
  }catch(e){}
  setTimeout(()=>waitForServer(n+1,oldVer),2000);
}
if(sessionStorage.getItem('upd-just-restarted')){
  sessionStorage.removeItem('upd-just-restarted');
  document.querySelector('nav button[data-s="settings"]').click();
}
init();
</script>
"#;
