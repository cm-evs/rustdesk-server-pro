// Web pages for the API service: client OIDC login page and the web console.
// Plain server-rendered HTML shells + vanilla JS calling the JSON APIs in api.rs.

pub const CSS: &str = r#"
*{box-sizing:border-box}body{margin:0;font-family:-apple-system,'Segoe UI',Roboto,'Helvetica Neue',Arial,sans-serif;background:#0f1419;color:#e6edf3}
a{color:#58a6ff}.wrap{max-width:1080px;margin:0 auto;padding:24px 16px}
.card{background:#161b22;border:1px solid #30363d;border-radius:10px;padding:20px;margin-bottom:16px}
h1{font-size:20px;margin:0 0 16px}h2{font-size:16px;margin:0 0 12px}
input,select{width:100%;padding:9px 10px;margin:6px 0 12px;background:#0d1117;border:1px solid #30363d;border-radius:6px;color:#e6edf3}
button{background:#1f6feb;color:#fff;border:0;border-radius:6px;padding:9px 16px;cursor:pointer;font-size:14px}
button:hover{background:#2270d4}button.danger{background:#b62324}button.danger:hover{background:#8e1b1c}button.ghost{background:#21262d;border:1px solid #30363d}
table{width:100%;border-collapse:collapse;font-size:14px}td,th{padding:8px;border-bottom:1px solid #21262d;text-align:left;vertical-align:top}
.err{color:#f85149;min-height:20px}.ok{color:#3fb950}.muted{color:#8b949e;font-size:13px}
.center{max-width:380px;margin:12vh auto}
.row{display:flex;gap:16px;flex-wrap:wrap}.row .card{flex:1;min-width:240px}
.stat{font-size:28px;font-weight:600}
nav{display:flex;gap:8px;flex-wrap:wrap;margin-bottom:16px}
nav button{background:#21262d;border:1px solid #30363d}nav button.active{background:#1f6feb;border-color:#1f6feb}
section{display:none}section.active{display:block}
.dl{display:flex;gap:8px;align-items:center}.dl input{margin:0;min-width:0}
table.gent{table-layout:fixed}
td.fname{word-break:break-all;font-size:12px;line-height:1.5}
td.nowrap{white-space:nowrap}
code{background:#0d1117;padding:2px 6px;border-radius:4px;font-size:13px}
.cap{display:flex;gap:8px;align-items:center}.cap img{height:54px;border:1px solid #30363d;border-radius:6px;background:#e8ecf1;display:block}.cap input{margin:0}
.capbtn{background:transparent;border:0;padding:0;cursor:pointer}.capbtn:hover{background:transparent}
.capq{font-size:18px;font-weight:600;white-space:nowrap}
.linkbtn{background:none;border:0;color:#58a6ff;padding:0;font-size:13px;cursor:pointer;text-decoration:underline}.linkbtn:hover{background:none}
"#;

fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"zh\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{title}</title><style>{CSS}</style></head><body>{body}</body></html>"
    )
}

pub fn login_page() -> String {
    page(
        "登录 - RustDesk 服务控制台",
        r#"<div class="center"><div class="card"><h1>RustDesk 服务控制台</h1>
<div class="err" id="err"></div>
<label>用户名</label><input id="u" autocomplete="username">
<label>密码</label><input id="p" type="password" autocomplete="current-password">
<label>验证码</label><div class="cap"><input id="c" autocomplete="off" placeholder="输入图片中的字符"><button type="button" id="capbtn" class="capbtn" title="点击刷新验证码" aria-label="刷新验证码"><img id="cap" src="/api/captcha" alt="图片验证码"></button><span id="capq" class="capq" style="display:none"></span></div>
<p class="muted"><button type="button" id="capmode" class="linkbtn">看不清？改用文字题</button></p>
<button id="btn" style="width:100%">登录</button>
<p class="muted">没有账号？<a href="/console/register">注册</a>（首个注册用户自动成为管理员）</p>
</div></div>
<script>
const cap=document.getElementById('cap');
const capq=document.getElementById('capq');
let textMode=false;
async function loadTextCap(){capq.textContent='加载中…';try{const r=await fetch('/api/captcha?mode=text');const j=await r.json();capq.textContent=j.question||'加载失败，请重试';}catch(e){capq.textContent='加载失败，请重试';}}
function refreshCap(){if(textMode){loadTextCap();}else{cap.src='/api/captcha?t='+Date.now();}}
document.getElementById('capbtn').onclick=refreshCap;
document.getElementById('capmode').onclick=e=>{e.preventDefault();textMode=!textMode;document.getElementById('c').value='';document.getElementById('c').placeholder=textMode?'输入计算结果':'输入图片中的字符';document.getElementById('capbtn').style.display=textMode?'none':'';capq.style.display=textMode?'':'none';e.target.textContent=textMode?'改用图片验证码':'看不清？改用文字题';refreshCap();};
document.getElementById('btn').onclick=async()=>{
  const btn=document.getElementById('btn'); if(btn.disabled)return; btn.disabled=true;
  try{
    const r=await fetch('/api/auth/login',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({username:u.value,password:p.value,captcha:c.value})});
    const j=await r.json();
    if(j.ok){location.href='/console';}else{document.getElementById('err').textContent=j.error||'登录失败';refreshCap();document.getElementById('c').value='';btn.disabled=false;}
  }catch(e){document.getElementById('err').textContent='网络错误，请检查连接后重试';btn.disabled=false;}
};
u.addEventListener('keydown',e=>{if(e.key==='Enter')btn.click();});
p.addEventListener('keydown',e=>{if(e.key==='Enter')btn.click();});
c.addEventListener('keydown',e=>{if(e.key==='Enter')btn.click();});
</script>"#,
    )
}

pub fn register_page() -> String {
    page(
        "注册 - RustDesk 服务控制台",
        r#"<div class="center"><div class="card"><h1>注册账号</h1>
<div class="err" id="err"></div>
<label>用户名</label><input id="u" autocomplete="username">
<label>密码（至少 6 位）</label><input id="p" type="password" autocomplete="new-password">
<label>验证码</label><div class="cap"><input id="c" autocomplete="off" placeholder="输入图片中的字符"><button type="button" id="capbtn" class="capbtn" title="点击刷新验证码" aria-label="刷新验证码"><img id="cap" src="/api/captcha" alt="图片验证码"></button><span id="capq" class="capq" style="display:none"></span></div>
<p class="muted"><button type="button" id="capmode" class="linkbtn">看不清？改用文字题</button></p>
<button id="btn" style="width:100%">注册</button>
<p class="muted">已有账号？<a href="/console/login">登录</a></p>
</div></div>
<script>
const cap=document.getElementById('cap');
const capq=document.getElementById('capq');
let textMode=false;
async function loadTextCap(){capq.textContent='加载中…';try{const r=await fetch('/api/captcha?mode=text');const j=await r.json();capq.textContent=j.question||'加载失败，请重试';}catch(e){capq.textContent='加载失败，请重试';}}
function refreshCap(){if(textMode){loadTextCap();}else{cap.src='/api/captcha?t='+Date.now();}}
document.getElementById('capbtn').onclick=refreshCap;
document.getElementById('capmode').onclick=e=>{e.preventDefault();textMode=!textMode;document.getElementById('c').value='';document.getElementById('c').placeholder=textMode?'输入计算结果':'输入图片中的字符';document.getElementById('capbtn').style.display=textMode?'none':'';capq.style.display=textMode?'':'none';e.target.textContent=textMode?'改用图片验证码':'看不清？改用文字题';refreshCap();};
document.getElementById('btn').onclick=async()=>{
  const btn=document.getElementById('btn'); if(btn.disabled)return; btn.disabled=true;
  try{
    const r=await fetch('/api/auth/register',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({username:u.value,password:p.value,captcha:c.value})});
    const j=await r.json();
    if(j.ok){location.href='/console';}else{document.getElementById('err').textContent=j.error||'注册失败';refreshCap();document.getElementById('c').value='';btn.disabled=false;}
  }catch(e){document.getElementById('err').textContent='网络错误，请检查连接后重试';btn.disabled=false;}
};
u.addEventListener('keydown',e=>{if(e.key==='Enter')btn.click();});
p.addEventListener('keydown',e=>{if(e.key==='Enter')btn.click();});
c.addEventListener('keydown',e=>{if(e.key==='Enter')btn.click();});
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
        r#"<div class="center"><div class="card"><h1>登录 RustDesk</h1>
<p class="muted">使用你的账号登录，登录后请返回 RustDesk 客户端。</p>
{err_html}
<form method="post" action="/oidc/login">
<input type="hidden" name="code" value="{code}">
<label>用户名</label><input name="username" autocomplete="username" required>
<label>密码</label><input name="password" type="password" autocomplete="current-password" required>
<label>验证码</label><div class="cap"><input name="captcha" id="c" autocomplete="off" placeholder="输入图片中的字符"><button type="button" id="capbtn" class="capbtn" title="点击刷新验证码" aria-label="刷新验证码"><img id="cap" src="/api/captcha" alt="图片验证码"></button><span id="capq" class="capq" style="display:none"></span></div>
<p class="muted"><button type="button" id="capmode" class="linkbtn">看不清？改用文字题</button></p>
<button style="width:100%">登录</button>
</form>
</div></div>"#
    );
    body.push_str(
        r#"<script>
const cap=document.getElementById('cap');
const capq=document.getElementById('capq');
let textMode=false;
async function loadTextCap(){capq.textContent='加载中…';try{const r=await fetch('/api/captcha?mode=text');const j=await r.json();capq.textContent=j.question||'加载失败，请重试';}catch(e){capq.textContent='加载失败，请重试';}}
function refreshCap(){if(textMode){loadTextCap();}else{cap.src='/api/captcha?t='+Date.now();}}
document.getElementById('capbtn').onclick=refreshCap;
document.getElementById('capmode').onclick=e=>{e.preventDefault();textMode=!textMode;document.getElementById('c').value='';document.getElementById('c').placeholder=textMode?'输入计算结果':'输入图片中的字符';document.getElementById('capbtn').style.display=textMode?'none':'';capq.style.display=textMode?'':'none';e.target.textContent=textMode?'改用图片验证码':'看不清？改用文字题';refreshCap();};
</script>"#,
    );
    page("登录 RustDesk", &body)
}

pub fn oidc_success_page() -> String {
    page(
        "登录成功",
        r#"<div class="center"><div class="card"><h1 class="ok">登录成功</h1>
<p>请返回 RustDesk 客户端，登录状态会自动完成。可以关闭此页面。</p>
</div></div>"#,
    )
}

pub fn console_page() -> String {
    page("RustDesk 服务控制台", CONSOLE_BODY)
}

const CONSOLE_BODY: &str = r#"
<div class="wrap">
<div style="display:flex;justify-content:space-between;align-items:center">
<h1>RustDesk 服务控制台</h1>
<div><span id="who" class="muted"></span> <button class="ghost" id="logout">退出</button></div>
</div>
<nav>
<button data-s="overview" class="active">概览</button>
<button data-s="devices">设备</button>
<button data-s="users" class="adminonly">用户管理</button>
<button data-s="tokens">API Token</button>
<button data-s="genclient">生成客户端</button>
<button data-s="settings" class="adminonly">服务器设置</button>
</nav>

<section id="s-overview" class="active">
<div class="row adminonly">
<div class="card"><div class="muted">注册用户</div><div class="stat" id="stat-users">-</div></div>
<div class="card"><div class="muted">已绑定设备</div><div class="stat" id="stat-devices">-</div></div>
<div class="card"><div class="muted">已注册终端（hbbs）</div><div class="stat" id="stat-peers">-</div></div>
</div>
<div class="err" id="ov-err"></div>
<div class="card"><h2>说明</h2>
<p class="muted">客户端在「设置 → 网络」填入本服务器地址，或直接使用「生成客户端」里预绑定配置的安装包，即可配合账号登录使用。设备在客户端登录账号后会通过心跳自动绑定到所属用户。</p>
</div>
</section>

<section id="s-devices">
<div class="card"><h2>设备</h2>
<p class="muted"><button class="ghost" id="dev-refresh">刷新</button> <span id="dev-updated"></span></p>
<div id="dev-list">加载中…</div></div>
</section>

<section id="s-users">
<div class="card"><h2>用户管理</h2><div id="user-list">加载中…</div></div>
</section>

<section id="s-tokens">
<div class="card"><h2>API Token（供第三方系统集成）</h2>
<p class="muted">请求带 <code>Authorization: Bearer rdt_...</code> 即可调用管理接口。Token 只在创建时显示一次。</p>
<div class="err" id="tok-err"></div><div class="ok" id="tok-new"></div>
<label>名称</label><input id="tok-name" placeholder="如：内部系统">
<button id="tok-create">创建 Token</button>
<div id="tok-list" style="margin-top:12px">加载中…</div></div>
</section>

<section id="s-genclient">
<div class="card"><h2>生成预绑定客户端</h2>
<p class="muted">生成的安装包已内置本服务器地址与密钥（编码进文件名，界面不显示服务器与密钥设置）。把下载链接发给用户即可，用户安装后直接登录账号使用。</p>
<div class="err" id="gen-err"></div>
<label>备注（可选）</label><input id="gen-label" placeholder="如：公司办公电脑">
<button id="gen-btn">生成 Windows 客户端</button>
<div id="gen-list" style="margin-top:12px">加载中…</div></div>
<div class="card"><h2>官方客户端手动配置</h2>
<p class="muted">已经装好官方 RustDesk 客户端的用户不用下载上面的安装包：在官方客户端「设置 → 网络」里填入下面四项，保存后即可登录本服务器的账号使用。下面显示的值与生成客户端内置的配置完全一致。</p>
<div id="cfg-body">加载中…</div></div>
</section>

<section id="s-settings">
<div class="card"><h2>服务器设置</h2>
<div class="err" id="set-err"></div><div class="ok" id="set-ok"></div>
<label>服务器公网地址（域名或 IP，不要带端口，生成客户端必填）</label>
<input id="set-host" placeholder="如 rd.example.com 或 203.0.113.10">
<label>API 地址覆盖（可空，默认 http://公网地址:21114）</label>
<input id="set-api" placeholder="可空">
<label>GitHub 加速镜像前缀（可空，下载官方客户端模板时优先使用；内置多个公共镜像自动轮询，最后直连）</label>
<input id="set-mirror" placeholder="如 https://gh-proxy.com">
<label style="display:flex;gap:8px;align-items:center"><input type="checkbox" id="set-hide" style="width:auto;margin:0"> 在客户端隐藏服务器/密钥设置（通过心跳下发）</label>
<label style="display:flex;gap:8px;align-items:center"><input type="checkbox" id="set-allowreg" style="width:auto;margin:0"> 允许开放注册（任何人可自助注册账号）</label>
<p class="muted">关闭后，只有管理员登录状态下打开注册页才能新建账号。公网部署建议关闭。</p>
<label style="display:flex;gap:8px;align-items:center"><input type="checkbox" id="set-allowrc" style="width:auto;margin:0"> 允许远程修改配置（通过心跳下发到所有客户端）</label>
<p class="muted">开启后，控制端可以操作被控端的 RustDesk 客户端界面与设置（包括改密码、改配置），不用再逐台到被控端本地勾选。仅在自有设备上开启；已在本机手动改过这一项的客户端以本机设置为准。</p>
<button id="set-save">保存设置</button></div>
<div class="card"><h2>监听端口（当前生效值）</h2>
<p class="muted" id="ports-line">读取中…</p>
<p class="muted">端口由启动参数决定，不在这里改：编辑程序目录下的 server.env（没有就从 server.env.example 复制一份），改 HBBS_PORT / HBBR_PORT / API_PORT / RELAY_SERVERS 后重启服务端。改端口后必须重新生成客户端（新客户端才会带新端口），并更新路由器端口转发；已生成的旧客户端会连不上新端口。换端口只能躲开针对默认端口的批量扫描，真正的安全仍靠服务器密钥与账号登录——知道新端口的人仍可注册 ID。</p></div>
<div class="card"><h2>系统服务（Ubuntu）</h2>
<p class="muted">在程序目录执行下面命令，把服务端注册为 systemd 服务：开机自启、崩溃自动重启。服务跑的就是 start.sh，所以 server.env 里的端口与中继配置同样生效；升级时只需替换 hbbs / hbbr 两个文件，不用重装服务。</p>
<div style="font-family:monospace;background:#0d1117;border:1px solid #30363d;border-radius:6px;padding:10px">sudo ./install-service.sh<br>sudo ./uninstall-service.sh&nbsp;&nbsp;# 移除服务<br>systemctl status rustdesk-server-pro<br>journalctl -u rustdesk-server-pro -f&nbsp;&nbsp;# 看服务日志</div></div>
<div class="card"><h2>官方客户端模板</h2>
<p class="muted" id="tpl-status">检查中…</p>
<label>版本（可空 = 最新版）</label><input id="tpl-ver" placeholder="如 1.4.2">
<button id="tpl-btn">下载/更新模板</button>
<div id="tpl-prog" style="display:none;margin-top:12px">
<div style="background:#0d1117;border:1px solid #30363d;border-radius:6px;height:14px;overflow:hidden">
<div id="tpl-bar" style="height:100%;width:0%;background:#1f6feb;transition:width .5s"></div>
</div>
<p class="muted" id="tpl-detail"></p>
</div>
<p class="muted">模板是官方 RustDesk Windows 安装程序，生成客户端时会在其基础上改文件名注入配置，不修改程序本身。下载会自动在多个 GitHub 加速镜像与直连之间切换（某个源太慢会自动换下一个）。</p></div>
</section>
</div>

<script>
const $=id=>document.getElementById(id);
let ME=null;
async function api(url,opts={}){const r=await fetch(url,opts);if(r.status===401){location.href='/console/login';throw new Error('unauth');}return r.json();}
async function copyText(t,btn){let ok=false;try{await navigator.clipboard.writeText(t);ok=true;}catch(e){const ta=document.createElement('textarea');ta.value=t;ta.style.position='fixed';ta.style.opacity='0';document.body.appendChild(ta);ta.select();try{ok=document.execCommand('copy');}catch(_){}ta.remove();}if(btn){const o=btn.textContent;btn.textContent=ok?'已复制':'复制失败';setTimeout(()=>btn.textContent=o,1200);}}
function esc(s){return String(s==null?'':s).replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));}
function loadFail(el,fnName,msg){el.innerHTML='<p class="err">加载失败'+(msg?('：'+esc(msg)):'')+'。<button class="ghost" onclick="'+fnName+'()">重试</button></p>';}
async function init(){
  try{ ME=await api('/api/me'); }catch(e){ if(e.message!=='unauth')$('who').textContent='加载失败，请检查网络后刷新页面'; return; }
  if(!ME.ok){location.href='/console/login';return;}
  $('who').textContent=ME.username+(ME.is_admin?'（管理员）':'');
  if(!ME.is_admin){document.querySelectorAll('.adminonly').forEach(e=>e.style.display='none');}
  loadOverview(); loadDevices();
}
document.querySelectorAll('nav button').forEach(b=>b.onclick=()=>{
  document.querySelectorAll('nav button').forEach(x=>x.classList.remove('active'));b.classList.add('active');
  document.querySelectorAll('section').forEach(x=>x.classList.remove('active'));$('s-'+b.dataset.s).classList.add('active');
  if(b.dataset.s==='overview')loadOverview(); if(b.dataset.s==='devices')loadDevices();
  if(b.dataset.s==='users')loadUsers(); if(b.dataset.s==='tokens')loadTokens();
  if(b.dataset.s==='genclient'){loadGen();loadCfg();} if(b.dataset.s==='settings')loadSettings();
});
$('dev-refresh').onclick=()=>loadDevices();
$('logout').onclick=async()=>{try{await api('/api/auth/logout',{method:'POST'});}catch(e){} location.href='/console/login';};

async function loadOverview(){
  if(!ME.is_admin)return;
  let j; try{ j=await api('/api/admin/overview'); }catch(e){ if(e.message!=='unauth')$('ov-err').textContent='概览加载失败，请刷新重试'; return; }
  if(!j.ok){ $('ov-err').textContent='概览加载失败：'+(j.error||'未知错误'); return; }
  $('ov-err').textContent='';
  $('stat-users').textContent=j.users; $('stat-devices').textContent=j.devices; $('stat-peers').textContent=j.peers;
}
async function loadDevices(){
  let j; try{ j=await api('/api/devices'); }catch(e){ if(e.message!=='unauth')loadFail($('dev-list'),'loadDevices'); return; }
  if(!j.ok){ loadFail($('dev-list'),'loadDevices',j.error); return; }
  $('dev-updated').textContent='更新于 '+new Date().toLocaleTimeString();
  if(!j.devices.length){$('dev-list').innerHTML='<p class="muted">暂无设备。设备在客户端登录账号后会出现在这里。</p>';return;}
  $('dev-list').innerHTML='<table><tr><th>设备 ID</th><th>状态</th><th>主机名</th><th>系统</th><th>所属用户</th><th>版本</th><th>最近在线</th></tr>'+
    j.devices.map(d=>`<tr><td>${esc(d.device_id)}</td><td>${(j.now-d.last_seen)<300?'<span class="ok">● 在线</span>':'<span class="muted">● 离线</span>'}</td><td>${esc(d.hostname||'')}</td><td>${esc(d.os||'')}</td><td>${d.username?esc(d.username):'<span class=muted>未绑定</span>'}</td><td>${esc(d.version||'')}</td><td>${new Date(d.last_seen*1000).toLocaleString()}</td></tr>`).join('')+'</table>';
}
let USERS=[];
async function loadUsers(){
  let j; try{ j=await api('/api/admin/users'); }catch(e){ if(e.message!=='unauth')loadFail($('user-list'),'loadUsers'); return; }
  if(!j.ok){ loadFail($('user-list'),'loadUsers',j.error); return; }
  USERS=j.users;
  $('user-list').innerHTML='<table><tr><th>用户名</th><th>角色</th><th>状态</th><th>注册时间</th><th>操作</th></tr>'+
    j.users.map((u,i)=>`<tr><td>${esc(u.username)}</td><td>${u.is_admin?'管理员':'普通'}</td><td>${u.disabled?'已禁用':'正常'}</td><td>${new Date(u.created_at*1000).toLocaleString()}</td>
    <td><button class="ghost" onclick="uact(USERS[${i}].username,'${u.disabled?'enable':'disable'}')">${u.disabled?'启用':'禁用'}</button>
    <button class="ghost" onclick="uact(USERS[${i}].username,'${u.is_admin?'unadmin':'admin'}')">${u.is_admin?'取消管理员':'设管理员'}</button>
    <button class="ghost" onclick="upwd(USERS[${i}].username)">重置密码</button>
    <button class="danger" onclick="uact(USERS[${i}].username,'delete')">删除</button></td></tr>`).join('')+'</table>';
}
async function uact(name,act){
  if(act==='delete'&&!confirm('删除用户 '+name+'？该账号将无法登录，其设备变为未绑定，名下 API Token 同时吊销；删除不可恢复。'))return;
  const j=await api('/api/admin/users/'+encodeURIComponent(name)+'/'+act,{method:'POST'}); if(j.ok)loadUsers(); else alert(j.error||'操作失败');
}
async function upwd(name){
  const pw=prompt('为 '+name+' 设置新密码（至少 6 位）'); if(!pw)return;
  const j=await api('/api/admin/users/'+encodeURIComponent(name)+'/password',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({password:pw})});
  alert(j.ok?'已重置':(j.error||'失败'));
}
let TOKENS=[];
async function loadTokens(){
  let j; try{ j=await api('/api/tokens'); }catch(e){ if(e.message!=='unauth')loadFail($('tok-list'),'loadTokens'); return; }
  if(!j.ok){ loadFail($('tok-list'),'loadTokens',j.error); return; }
  TOKENS=j.tokens;
  $('tok-list').innerHTML=j.tokens.length?'<table><tr><th>名称</th><th>创建时间</th><th>最近使用</th><th></th></tr>'+
    j.tokens.map((t,i)=>`<tr><td>${esc(t.name)}</td><td>${new Date(t.created_at*1000).toLocaleString()}</td><td>${t.last_used_at?new Date(t.last_used_at*1000).toLocaleString():'-'}</td><td><button class="danger" onclick="tdel(${i})">吊销</button></td></tr>`).join('')+'</table>':'<p class="muted">暂无 Token</p>';
}
$('tok-create').onclick=async()=>{
  const btn=$('tok-create'); if(btn.disabled)return; btn.disabled=true;
  try{
    const j=await api('/api/tokens',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:$('tok-name').value})});
    if(j.ok){$('tok-err').textContent='';$('tok-new').innerHTML='新 Token（只显示一次，请保存）：<div class="dl" style="margin-top:6px"><input readonly id="tok-new-val" value="'+esc(j.token)+'"><button class="ghost" onclick="copyText(document.getElementById(\'tok-new-val\').value,this)">复制</button></div>'; loadTokens();}else{$('tok-err').textContent=j.error||'失败';}
  }catch(e){ if(e.message!=='unauth')$('tok-err').textContent='网络错误，请重试'; }
  btn.disabled=false;
};
async function tdel(i){
  const t=TOKENS[i]; if(!t)return;
  if(!confirm('吊销 Token「'+t.name+'」？使用该 Token 的程序将立即无法访问，吊销不可恢复。'))return;
  const j=await api('/api/tokens/'+t.id,{method:'DELETE'}); if(j.ok)loadTokens(); else alert(j.error||'吊销失败');
}
let GEN=[];
async function loadGen(){
  let j; try{ j=await api('/api/admin/gen-clients'); }catch(e){ if(e.message!=='unauth')loadFail($('gen-list'),'loadGen'); return; }
  if(!j.ok){ loadFail($('gen-list'),'loadGen',j.error); return; }
  GEN=j.clients;
  $('gen-list').innerHTML=j.clients.length?'<table class="gent"><colgroup><col style="width:24%"><col style="width:9%"><col style="width:10%"><col style="width:12%"><col style="width:37%"><col style="width:8%"></colgroup><tr><th>文件</th><th>备注</th><th>生成者</th><th>生成时间</th><th>下载链接</th><th></th></tr>'+
    j.clients.map((c,i)=>`<tr><td class="fname">${esc(c.filename)}</td><td>${esc(c.label||'')}</td><td>${esc(c.created_by||'')}</td><td class="nowrap">${new Date(c.created_at*1000).toLocaleString()}</td>
    <td><div class="dl"><input readonly value="${esc(c.download_url)}"><button class="ghost" onclick="copyText(this.closest('.dl').querySelector('input').value,this)">复制</button><button class="ghost" onclick="window.open(this.closest('.dl').querySelector('input').value,'_blank')">下载</button></div></td>
    <td class="nowrap"><button class="danger" onclick="gdel(${i})">删除</button></td></tr>`).join('')+'</table>':'<p class="muted">尚未生成。'+(ME.is_admin?'先在「服务器设置」里填公网地址并下载模板。':'生成前需管理员先在服务器设置里填公网地址并下载模板。')+'</p>';
}
async function loadCfg(){
  let j; try{ j=await api('/api/client-config'); }catch(e){ if(e.message!=='unauth')loadFail($('cfg-body'),'loadCfg'); return; }
  if(!j.ok){ loadFail($('cfg-body'),'loadCfg',j.error); return; }
  if(!j.configured){ $('cfg-body').innerHTML='<p class="muted">'+(ME.is_admin?'尚未在「服务器设置」里填写公网地址，填好后这里会显示客户端配置。':'管理员尚未填写服务器公网地址，暂时无法显示客户端配置。')+'</p>'; return; }
  const row=(label,val)=>'<label>'+label+'</label><div class="dl"><input readonly value="'+esc(val)+'"><button class="ghost" onclick="copyText(this.closest(\'.dl\').querySelector(\'input\').value,this)">复制</button></div>';
  $('cfg-body').innerHTML=row('ID 服务器',j.host)+row('中继服务器',j.relay)+row('API 服务器',j.api)+row('密钥（Key）',j.key);
}
$('gen-btn').onclick=async()=>{
  const btn=$('gen-btn'); if(btn.disabled)return; btn.disabled=true;
  try{
    const j=await api('/api/admin/gen-client',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({label:$('gen-label').value})});
    if(j.ok){$('gen-err').textContent='';loadGen();}else{$('gen-err').textContent=j.error||'生成失败';}
  }catch(e){ if(e.message!=='unauth')$('gen-err').textContent='网络错误，请重试'; }
  btn.disabled=false;
};
async function gdel(i){
  const c=GEN[i]; if(!c)return;
  if(!confirm('删除生成的客户端「'+(c.label||c.filename)+'」？删除后，已发出的下载链接立即失效，且不可恢复。'))return;
  const j=await api('/api/admin/gen-clients/'+c.id,{method:'DELETE'}); if(j.ok)loadGen(); else alert(j.error||'删除失败');
}
async function loadSettings(){
  let j; try{ j=await api('/api/admin/settings'); }catch(e){ if(e.message!=='unauth')$('set-err').textContent='加载设置失败，请刷新重试'; return; }
  if(!j.ok){ $('set-err').textContent='加载设置失败：'+(j.error||'未知错误'); return; }
  $('set-host').value=j.settings.public_host||''; $('set-api').value=j.settings.api_url_override||'';
  $('set-mirror').value=j.settings.github_mirror||'';
  $('set-hide').checked=!!j.settings.hide_server_settings;
  $('set-allowreg').checked=j.settings.allow_register!==false;
  $('set-allowrc').checked=!!j.settings.allow_remote_config;
  if(j.ports){let pl='中继/ID：'+j.ports.rendezvous+'（NAT 测试 '+j.ports.nat_test+'，WebSocket '+j.ports.ws_rendezvous+'）｜中继：'+j.ports.relay+'（WebSocket '+j.ports.ws_relay+'）｜API：'+j.ports.api; if(j.ports.console){pl+='｜控制台（独立监听）：'+j.ports.console_listen;}else{pl+='（与 API 同端口）';} if(j.ports.tls){pl+='｜HTTPS：已启用';} $('ports-line').textContent=pl;}
  $('tpl-status').textContent=j.template_ready?('已就绪：'+(j.template_version||'')):'未下载（生成客户端前需要先下载）';
  pollTpl();
}
$('set-save').onclick=async()=>{
  const btn=$('set-save'); if(btn.disabled)return; btn.disabled=true;
  $('set-ok').textContent=''; $('set-err').textContent='';
  try{
    const j=await api('/api/admin/settings',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({public_host:$('set-host').value,api_url_override:$('set-api').value,github_mirror:$('set-mirror').value,hide_server_settings:$('set-hide').checked,allow_register:$('set-allowreg').checked,allow_remote_config:$('set-allowrc').checked})});
    if(j.ok){$('set-ok').textContent='已保存';}else{$('set-err').textContent=j.error||'失败';}
  }catch(e){ if(e.message!=='unauth')$('set-err').textContent='网络错误，请重试'; }
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
    $('tpl-status').textContent='正在下载官方客户端模板'+(j.version?(' '+j.version):'');
    $('tpl-detail').textContent=(j.total>0?(fmtMB(j.downloaded)+' / '+fmtMB(j.total)+'（'+pct.toFixed(1)+'%）'):fmtMB(j.downloaded))+
      ' · 速度 '+fmtSpeed(tplSpeed)+(j.source?(' · 来源：'+j.source):'');
    tplTimer=setTimeout(pollTpl,1000);
  }else{
    $('tpl-btn').disabled=false;
    if(j.state==='done'){
      $('tpl-bar').style.width='100%';
      $('tpl-detail').textContent='下载完成';
      $('tpl-status').textContent='已就绪：'+(j.template_version||j.version||'');
    }else if(j.state==='error'){
      $('tpl-status').textContent='下载失败：'+(j.error||'未知错误');
      $('tpl-detail').textContent='';
    }else if(j.template_ready){
      $('tpl-status').textContent='已就绪：'+(j.template_version||'');
    }
    if(tplTimer){clearTimeout(tplTimer);tplTimer=null;}
  }
}
$('tpl-btn').onclick=async()=>{
  tplLastBytes=0; tplLastTime=0; tplSpeed=0;
  const j=await api('/api/admin/template/download',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({version:$('tpl-ver').value})});
  if(!j.ok){$('tpl-status').textContent='启动下载失败：'+(j.error||'');return;}
  $('tpl-prog').style.display='block'; $('tpl-status').textContent='正在下载…';
  pollTpl();
};
init();
</script>
"#;
