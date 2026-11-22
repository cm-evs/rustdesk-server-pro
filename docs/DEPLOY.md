# RustDesk 服务端（账号增强版）部署与使用说明

基于官方开源 rustdesk-server（hbbs/hbbr 1.1.17）改造，增加了官方开源版没有的：

- **账号系统**：用户注册/登录，官方 RustDesk 客户端可直接用账号登录
- **Web 管理后台**：用户管理、设备列表、API Token、服务器设置
- **生成客户端**：后台一键生成预绑定本服务器的 Windows 客户端安装包，
  客户端界面不显示服务器地址与密钥，生成后提供可分发的下载链接
- **集成 Token**：`Authorization: Bearer rdt_...` 调用管理接口

数据库沿用官方的 SQLite（设备库 `db_v2.sqlite3` + 账号库 `api_v1.sqlite3`）。

**平台要求**：预编译包在 Ubuntu 24.04（glibc 2.39）上构建，需要
glibc ≥ 2.39 级别的系统（Ubuntu 24.04 / Debian 13 等）。更老的系统
请用随附源码包自行构建：在目标机安装 Rust 后进入源码目录执行
`cargo build --release`，产物在 `target/release/{hbbs,hbbr}`。

## 快速开始（Linux）

```bash
unzip rustdesk-server-pro-linux-*.zip
cd rustdesk-server-pro-linux-*
./start.sh          # 前台运行；或用 systemd/守护方式见下文
```

首次启动后打开 `http://<服务器IP>:21114/console/register` 注册账号，
**第一个注册的用户自动成为管理员**。

然后在管理后台：
1. 「服务器设置」填入服务器**公网地址**（域名或公网 IP）
2. 「服务器设置」点「下载/更新模板」（拉取官方最新版 Windows 客户端；
   页面会实时显示进度与速度。国内网络默认走多个 GitHub 加速镜像
   自动轮换，某个源太慢会停滞检测后自动换下一个，最后才直连；
   也可在设置里填自己的镜像前缀优先使用）
3. 「生成客户端」点生成 → 复制**下载链接**发给用户
4. 用户下载安装后，打开客户端用注册的账号登录即可使用

## 需要开放的端口

| 端口 | 协议 | 用途 |
|---|---|---|
| 21114 | TCP | API + Web 管理后台（本项目新增） |
| 21115 | TCP | NAT 类型测试 |
| 21116 | TCP/UDP | ID/信令服务（UDP 必须开） |
| 21117 | TCP | 中继服务 hbbr |
| 21118 | TCP | WebSocket（Web 客户端） |
| 21119 | TCP | 中继 WebSocket |

## Docker 部署

> [!WARNING]
> **Docker 用户升级到 v46.1 前必须先迁移数据。** v46 及更早的镜像把运行时数据写进了容器内的 `/usr/bin`（`/data` 卷始终为空）；直接拉新镜像重建容器，旧数据全部丢失：密钥变了，已配好的客户端连不上；账号库丢了，账号与 API key 全没了。先把旧容器里的数据拷出来，再重建：
>
> 一体镜像（容器名 `rustdesk-pro`，卷名 `rustdesk-pro-data`）：
>
> ```bash
> docker stop rustdesk-pro
> mkdir -p /tmp/rustdesk-migrate
> docker cp rustdesk-pro:/usr/bin/db_v2.sqlite3 /tmp/rustdesk-migrate/
> docker cp rustdesk-pro:/usr/bin/api_v1.sqlite3 /tmp/rustdesk-migrate/
> docker cp rustdesk-pro:/usr/bin/id_ed25519 /tmp/rustdesk-migrate/
> docker cp rustdesk-pro:/usr/bin/id_ed25519.pub /tmp/rustdesk-migrate/
> docker cp rustdesk-pro:/usr/bin/logs /tmp/rustdesk-migrate/
> docker run --rm -v rustdesk-pro-data:/data -v /tmp/rustdesk-migrate:/migrate \
>   busybox sh -c 'cp /migrate/db_v2.sqlite3 /migrate/api_v1.sqlite3 /migrate/id_ed25519 /migrate/id_ed25519.pub /data/ && cp -r /migrate/logs /data/'
> docker pull ghcr.io/wirh/rustdesk-server-pro:v46.1
> docker rm rustdesk-pro
> # 按下文「一体镜像」命令重建容器，镜像标签用 :v46.1（预览版不移动 latest）
> ```
>
> 经典镜像（compose，容器名 `hbbs` / `hbbr`，数据目录 `./data`）：
>
> ```bash
> docker compose stop
> docker cp hbbs:/usr/bin/db_v2.sqlite3 ./data/
> docker cp hbbs:/usr/bin/api_v1.sqlite3 ./data/
> docker cp hbbs:/usr/bin/id_ed25519 ./data/
> docker cp hbbs:/usr/bin/id_ed25519.pub ./data/
> docker cp hbbs:/usr/bin/logs ./data/
> docker cp hbbr:/usr/bin/logs/hbbr.log ./data/logs/
> # 预览版不移动 latest 标签：把 docker-compose.yml 里的两个镜像标签改成 :v46.1
> docker compose pull
> docker compose up -d
> ```
>
> 旧容器已删则数据无法找回。完整说明见下文「升级到 v46.1（数据迁移）」。

两个镜像，版本标签与 Release 一致（`v45`、`latest` 等），支持 amd64 / arm64 / armv7 / i386：

| 镜像 | 说明 |
|---|---|
| `ghcr.io/wirh/rustdesk-server-pro`（Docker Hub：`tesx/rustdesk-server-pro`） | 一体镜像：s6 托管 hbbs + hbbr 两个服务 |
| `ghcr.io/wirh/rustdesk-server-pro-classic`（Docker Hub：`tesx/rustdesk-server-pro-classic`） | 经典镜像：只有 hbbs、hbbr 两个二进制，一个容器跑一个服务，配合 `docker-compose.yml` 使用 |

### 一体镜像（推荐）

```bash
docker run -d --name rustdesk-pro \
  -p 21114:21114 -p 21115:21115 -p 21116:21116 -p 21116:21116/udp \
  -p 21117:21117 -p 21118:21118 -p 21119:21119 \
  -v rustdesk-pro-data:/data \
  -e RELAY=<服务器公网地址> \
  ghcr.io/wirh/rustdesk-server-pro:latest
```

- 数据存在 `/data` 卷里：密钥（`id_ed25519`、`id_ed25519.pub`）、账号库（`db_v2.sqlite3`、`api_v1.sqlite3`）与日志（`logs/`）。删容器不丢数据，卷删了才丢
- `RELAY` 填服务器公网地址（域名或公网 IP），中继地址由此分发
- `ENCRYPTED_ONLY=1`：只接受带密钥的加密连接（hbbs/hbbr 加 `-k _`）
- 首次启动后打开 `http://<服务器IP>:21114/console/register` 注册账号，第一个注册的用户自动成为管理员，后续步骤同上文「快速开始」

### 经典镜像（compose）

仓库根目录的 `docker-compose.yml` 即是现成配置：hbbs、hbbr 各一个容器，共用 `./data`。容器内工作目录是 `/root`，数据（密钥、账号库、日志）落在挂载的 `./data` 里。把文件里的 `rustdesk.example.com` 改成服务器公网地址后：

```bash
docker compose up -d
```

### 升级

拉新镜像后重建容器即可，数据在卷里不受影响：

```bash
docker pull ghcr.io/wirh/rustdesk-server-pro:latest
docker compose up -d   # 或 docker run 重建
```

Docker 部署不走控制台一键更新（一键更新只换宿主机上的 hbbs 二进制）。

### 升级到 v46.1（数据迁移）

v46 及更早的镜像有个缺陷：hbbs/hbbr 启动时把工作目录锚到二进制所在目录 `/usr/bin`，运行时数据（密钥、账号库、日志）写进了容器内的 `/usr/bin`，`/data` 卷（经典镜像是 `./data`）始终为空。升级到 v46.1 前，先把旧容器里的数据拷出来，否则重建容器后数据丢失：密钥变了，已配好的客户端连不上；账号库丢了，账号与 API key 全没了。旧容器已删则数据无法找回。

一体镜像（容器名 `rustdesk-pro`，卷名 `rustdesk-pro-data`）：

```bash
docker stop rustdesk-pro
mkdir -p /tmp/rustdesk-migrate
docker cp rustdesk-pro:/usr/bin/db_v2.sqlite3 /tmp/rustdesk-migrate/
docker cp rustdesk-pro:/usr/bin/api_v1.sqlite3 /tmp/rustdesk-migrate/
docker cp rustdesk-pro:/usr/bin/id_ed25519 /tmp/rustdesk-migrate/
docker cp rustdesk-pro:/usr/bin/id_ed25519.pub /tmp/rustdesk-migrate/
docker cp rustdesk-pro:/usr/bin/logs /tmp/rustdesk-migrate/
docker run --rm -v rustdesk-pro-data:/data -v /tmp/rustdesk-migrate:/migrate \
  busybox sh -c 'cp /migrate/db_v2.sqlite3 /migrate/api_v1.sqlite3 /migrate/id_ed25519 /migrate/id_ed25519.pub /data/ && cp -r /migrate/logs /data/'
docker pull ghcr.io/wirh/rustdesk-server-pro:v46.1
docker rm rustdesk-pro
# 按上文「一体镜像」命令重建容器，镜像标签改成 :v46.1（预览版不移动 latest）
```

用 `-v ./data:/data` 绑定挂载的，直接把文件拷进宿主的 `./data` 目录，无需临时容器。

经典镜像（compose，容器名 `hbbs` / `hbbr`，数据目录 `./data`）：

```bash
docker compose stop
docker cp hbbs:/usr/bin/db_v2.sqlite3 ./data/
docker cp hbbs:/usr/bin/api_v1.sqlite3 ./data/
docker cp hbbs:/usr/bin/id_ed25519 ./data/
docker cp hbbs:/usr/bin/id_ed25519.pub ./data/
docker cp hbbs:/usr/bin/logs ./data/
docker cp hbbr:/usr/bin/logs/hbbr.log ./data/logs/
# 预览版不移动 latest 标签：把 docker-compose.yml 里的两个镜像标签改成 :v46.1
docker compose pull
docker compose up -d
```

hbbr 容器里只有日志（默认不启用中继密钥校验，不生成密钥），数据库与密钥只从 hbbs 容器拷。拷完确认 `./data` 里有 `db_v2.sqlite3`、`api_v1.sqlite3`、`id_ed25519`、`id_ed25519.pub` 再重建容器。

### 本地构建镜像

```bash
./docker/build-local.sh [--classic] [--tag TAG] [发布包或解包目录]
```

默认用 `./dist-pro` 里最新的 linux-x86_64-glibc 发布包。

## hbbs 新增参数

```
--api-listen [::]:21114   API/后台监听地址（默认即此值；off 关闭）
--api-db api_v1.sqlite3      账号数据库文件（默认服务端目录，见 v21）
```

环境变量 `TEST_HBBS=no`：跳过 hbbs 启动时的自连测试。在受限网络
（防火墙禁止服务器自连 21116 等）里自检会失败并导致 hbbs 退出，
此时务必带上这个环境变量，本包的 start.sh 已默认设置。

## 与官方客户端的配合

- 已装官方客户端的用户：客户端「设置 → 网络 → ID/中继服务器」填
  本服务器地址、密钥填服务器公钥（管理后台服务器设置页可查
  `id_ed25519.pub` 内容由服务端自动读取），API 服务器填
  `http://<公网地址>:21114`，即可用账号登录。
- 推荐用「生成客户端」分发的安装包：配置已预绑定，免配置、不显示
  服务器与密钥信息，且登录后通过心跳下发策略隐藏服务器设置区
  （可在「服务器设置」里关闭该行为）。普通用户也可以在控制台
  生成/下载属于自己的客户端（只能看到和删除自己生成的记录）；
  服务器设置、模板下载、用户管理仍仅管理员可见。设备在客户端
  登录账号后通过心跳自动绑定到所属用户，普通用户在「设备」页
  只能看到自己名下的设备。

## v5 修复：1.5.0 客户端远控握手超时

- 现象：客户端已注册、显示就绪，但发起远控时报
  `Failed to secure tcp: deadline has elapsed`。
- 原因：1.5.0 客户端在配置了服务器密钥且已登录账号时，连接
  中继服务器（21116 TCP）后会先等服务端发密钥交换消息；本
  服务端基于官方开源版（早于密钥交换功能），TCP 接入后从不
  主动先说话，客户端等满超时后报错。未配置密钥或未登录的
  客户端不走这条握手，因此不受影响。
- 修复：hbbs 在 TCP 接入时先发一条空的问候消息，客户端按
  官方文档的旧服务端兼容逻辑继续以明文走中继握手（与密钥
  交换功能出现之前的行为一致）。注意：中继通道本身因此不
  加密，但两端远控会话仍通过端到端密钥交换加密，不受影响。
  WebSocket 接入不受此问题影响（客户端会跳过该握手）。

## v7：控制台登录/注册验证码

- 网页控制台的登录页与注册页增加了图片验证码（服务端生成 SVG，
  5 位字符，5 分钟有效，一次性使用）。验证码只以哈希形式存服务端，
  浏览器只持有 challenge id，页面源码和 Cookie 里看不到答案。
- 验证码在密码校验之前验证：验证码错误时不透露账号密码是否正确；
  失败后图片自动刷新（一次性验证码用过即废）。
- 注意：客户端 App 内的登录（`/api/login`）没有加验证码——官方客户
  端没有验证码输入界面，加了会断掉客户端登录；OIDC 浏览器登录页
  也未改动（它是客户端登录流程的一环）。验证码是基础防机器人
  手段（防自动化撞库），防不住专业 OCR；需要更强验证时再接
  第三方验证服务或短信/邮箱码。

## v6 修复：主页“FormatException”报错

- 现象：客户端主页下方显示 `FormatException: Unexpected end of input`，
  点「再试」也一样（即使已登录）。
- 原因：客户端主页每次启动都会 POST `/api/currentUser` 刷新登录用户，
  且对返回体做无保护的 JSON 解析；本服务端之前没有这个接口，
  404 空包直接触发解析异常。设备组与通讯录接口本身一直正常，
  报错的其实是主页的用户刷新部件，不是设备组页。
- 修复：新增 `POST /api/currentUser`（Bearer 登录 token 鉴权），
  返回当前用户的 `name / display_name / status / is_admin` 等字段；
  未登录或 token 无效时返回 401 JSON，绝不返回空包。

## v9：客户端设备列表按登录账号隔离

- 现象：客户端设备组列表里出现其他账号的设备（管理员登录时
  能看到所有用户的设备，乃至只注册、未登录过账号的设备）。
- 原因：客户端用的 `/api/peers`（设备列表）与 `/api/users`（左
  侧用户栏）对管理员返回全量：全部已绑定设备，外加未绑定
  设备的合并项。
- 修复：这两个客户端接口改为始终只返回调用者自己账号下的
  数据，管理员也不例外。左侧用户栏因此也只显示自己——否
  则点选其他用户只会得到空列表。
- 不变：网页控制台的设备页（`/api/devices`）仍是管理员全量
  视图；普通用户在控制台也只看得到自己的设备。升级只需
  替换 hbbs，数据库与密钥不动。

## v10：监听端口自定义 + Ubuntu 系统服务

### 监听端口
- 原理：hbbs 的 `-p`（默认 21116）决定中继/ID 端口（附带 NAT
  测试端口 -1、WebSocket +2）；hbbr 的 `-p`（默认 21117）决定
  中继端口（WebSocket +2）；hbbs 的 `--api-listen`（默认
  [::]:21114）决定 API/控制台端口。客户端连接时，带端口
  的地址优先于默认端口，所以生成客户端会把当前生效端口嵌进
  配置里。
- 改法：编辑程序目录下的 server.env（没有就从
  server.env.example 复制一份），改 HBBS_PORT / HBBR_PORT /
  API_PORT / RELAY_SERVERS 后重启。RELAY_SERVERS 里的端口必须
  和 HBBR_PORT 一致，否则中继握手会找错端口。
- 改端口后必须做的三件事：重新生成客户端（旧客户端仍连旧
  端口）、更新路由器端口转发、确认启动日志里的监听端口。
  控制台「服务器设置」页会显示当前生效的全部端口。
- 提醒：换端口只能躲开针对默认端口的批量扫描；知道新端口的
  人仍可注册 ID，真正的控制仍需服务器密钥与账号登录。
- 升级注意：v10 的 start.sh 改成了读 server.env 的参数化版本。
  升级只换 hbbs / hbbr 两个文件，不要直接解压覆盖整个目录，
  否则会丢掉你改过的 start.sh。一次性迁移：把旧 start.sh 里
  的 --relay-servers 值写进 server.env，之后 start.sh 本身就
  不再重要，升级覆盖它也没关系。

### Ubuntu 系统服务
- 在程序目录执行 `sudo ./install-service.sh`：注册
  rustdesk-server-pro.service，开机自启、崩溃自动重启。服务
  跑的就是 start.sh，server.env 的配置同样生效。
- 安装前请先停掉手动启动的实例（否则端口冲突，安装脚本会
  直接拒绝）。
- 移除：`sudo ./uninstall-service.sh`。查看状态：
  `systemctl status rustdesk-server-pro`；看日志：
  `journalctl -u rustdesk-server-pro -f`。

## v11：禁止本机连接
- 现象：以前在客户端里输入自己的 ID 也能连上（本机弹出接
  受/密码框），现在服务端直接拒绝，客户端显示“不能连接本
  机”。
- 原理：连接请求（PunchHoleRequest）里带登录 token，服务端
  用它反查账号库的 client_tokens 得到发请求的设备 ID；如果
  等于目标 ID，就是连自己，直接拒绝，不会把请求转发给本机。
  直连和中继都从这个请求开始，所以两条路都拦住了。
- 注意：列表里本机那一项还是能点（灰掉需要改客户端界面，
  那是另一个大工程），但点了会立刻失败，不会再弹出密码框。
- 边界：没登录的客户端没有 token，服务端认不出是不是本机，
  拦不住（照常走老流程）。已登录的生成客户端都带 token，
  都能拦住。
- 升级：只换 hbbs（hbbr 不动），v11 含 v10 全部内容。

## v13：管理后台全面切换到 GoAdmin，Rust 旧控制台下线

- 变化：管理后台从 Rust hbbs 内嵌控制台切换到独立 GoAdmin 服务
  （`rustdesk-server-pro-admin`，默认监听 **21113**）。Rust hbbs 只保留
  客户端协议接口（登录、设备列表、地址簿、OIDC、心跳绑定）与 token
  化下载（`/dl/`）；旧控制台页面与管理接口全部下线：`/console`、
  `/console/login`、`/console/register`、`/api/auth/*`、`/api/captcha`、
  `/api/me`、`/api/admin/*`、`/api/devices`、`/api/tokens`、模板与生成
  客户端管理接口。访问旧地址返回 404；根路径 `/` 改为纯文本提示，
  指向 21113 的 `/admin`。
- 切换步骤（按顺序）：
  1. 停 hbbs。
  2. 换 hbbs 二进制（v13 包里的 hbbs；hbbr 未改，不用换）。
  3. 对 `api_v1.sqlite3` 执行 schema SQL（幂等，可重复执行）：
     `sqlite3 api_v1.sqlite3 < goadmin-schema-sqlite.sql`（管理端包里带）。
  4. 起 admin 服务（管理端包里的 `start-admin.sh`：默认监听
     `0.0.0.0:21113`，`-db` 指向与 hbbs 共享的 `api_v1.sqlite3`，
     `-clients-dir` 与 hbbs 共用目录，`GIN_MODE=release`）。
  5. 起 hbbs。
- 管理登录改走 `http://<服务器地址>:21113/admin`；初始账号
  **admin/admin，首次登录必须改掉**。注意：GoAdmin 登录账号与客户端
  登录账号（`users` 表）是两套；客户端登录账号不受影响（原有
  原有测试账号照常用）。
- 生成客户端链接改由 GoAdmin 出（在 `/admin` 里生成；下载仍走 `/dl/`
  token 链接）。老 `/dl/` 链接在 21114 上继续可用（hbbs 保留了 `/dl/`，
  与 GoAdmin 共用 clients 目录）。
- 模板需手动放置：模板自动下载随旧控制台下线而暂停；把官方客户端
  exe 放到 `-template` 路径（默认 `client-templates/rustdesk-windows.exe`）
  再生成。
- 验证码已随旧控制台下线：GoAdmin 登录目前无验证码。
- 设备实时在线状态：GoAdmin 只能看 `last_seen`，实时在线状态在
  hbbs 运行时里。
- 账号创建：旧版 `/console/register` 注册入口已下线；账号改由
  GoAdmin 管理员在 users 表里创建，再调密码设置接口设密码后即可登录。
- rdt_ 令牌：新发改由 GoAdmin 的 `POST /admin/api/tokens`（body
  username + name，原始 token 只返回一次）。注意：v13 的 Rust 侧不再
  提供 rdt_ 鉴权的管理接口（原接口已下线），外部工具如用 rdt_ 调过旧
  管理接口，需改走 GoAdmin 会话鉴权，直到 rdt_ 鉴权补到 GoAdmin 为止。
- 端口补充：21113（TCP，GoAdmin 管理后台）需另外开放/转发（不在原来
  21114–21119 转发段里）；21114 现在只剩客户端协议 API 与 `/dl/`。
- 回滚办法：换回 v12 的 hbbs 二进制并重启，旧控制台即恢复。库是共享
  的：v13 期间 GoAdmin 创建的用户、生成的客户端、改的设置都在同一
  个库里，旧控制台能看到；新 hbbs 不写 `web_sessions`/`captcha_codes`
  表（schema 留着不动），旧 hbbs 恢复后继续写，数据不受影响。
- 升级：v13 含 v12 全部内容（Windows 包同批）。

## v14（仅管理端）：GoAdmin 登录验证码

- 管理端包 v14（服务端包仍是 v13，不用换）：GoAdmin 登录页加了验证码。登录提交走 `POST /admin/signin`，验证码在密码检查之前校验：SVG 图片、服务端只存 sha256、5 分钟过期、一次有效、失败不验证密码；登录页点图片可刷新，失败自动刷新。
- 升级管理端：只换管理端包里的 `rustdesk-server-pro-admin` 二进制并重启 admin 服务（`start-admin.sh` 不变）；库不用重导 schema（验证码存在进程内存，不占表）。
- 注意：验证码是基础防自动化，不是 OCR-proof；未做频率限制/账号锁定；初始 admin/admin 首次登录仍须改掉。

## Windows 部署（两包合用）

- 两个包：`rustdesk-server-pro-windows-x86_64-v16.zip`（hbbs.exe、hbbr.exe、start.bat、start.ps1、server.env.example、check-windows-v13.ps1）与 `rustdesk-server-pro-admin-windows-x86_64-v17.zip`（rustdesk-server-pro-admin.exe、start-admin.bat、start-admin.ps1、goadmin-schema-sqlite.sql、src/）。**两包解到同一个目录部署**（把管理端包里的文件移到服务端目录），这样服务端与管理端共享同一个 `api_v1.sqlite3` 和 `clients` 目录，老 `/dl/` 链接继续可用。
- v16 修正（仅脚本编码，程序不变）：三个 .ps1 改为 UTF-8 带 BOM——v15 的脚本是无 BOM 的 UTF-8，Windows PowerShell 5.1 会按系统 ANSI（中文系统即 GBK）误读中文，导致解析失败；v16 直接覆盖旧脚本即可。
- 启动顺序：先双击 `start.bat`（服务端：hbbr 独立窗口 + 前台 hbbs，退出时自动停 hbbr），再双击 `start-admin.bat`（管理端，默认监听 `0.0.0.0:21113`）。首次运行防火墙弹窗要放行 21113–21119。
- 对外中继：先复制 `server.env.example` 为 `server.env` 并填入 `RELAY_SERVERS=<公网地址>:<中继端口>`（端口须与 HBBR_PORT 一致），再启动；不填则外网控制器报“无法连接到中继服务器”。端口默认 HBBS 21116、HBBR 21117、API 21114，可在 server.env 里改。
- Schema：管理端首次启动时自动导入 GoAdmin 系统表（内嵌 `goadmin-schema-sqlite.sql`，表已存在则跳过），不用装 sqlite3 CLI；手动导入仍是备用办法。
- 管理登录：`http://<服务器>:21113/admin`（登录带验证码，初始 admin/admin 首次登录必须改掉）。冒烟用 `check-windows-v13.ps1`（只读）：三进程、21113 + 21114–21119 监听、登录页验证码、旧控制台 404、根路径提示、关键文件、中继提醒。
- 回滚办法：停掉两个服务，换回旧版二进制并重启即可；库是共享的，新版期间的数据旧版照样看得到。

## v17（仅管理端）：/admin 跳转到登录页

- GoAdmin 本身只挂 `/admin/*` 下的页面（登录、鉴权、信息页），裸 `/admin` 路径没有任何 handler，浏览器打开会是 404——这不是部署坏了，是 GoAdmin 的默认行为。v17 起管理端把 `/admin` 302 跳转到 `/admin/login`，文档里的登录地址直接可用。
- 管理端包 Linux 与 Windows 同步为 v17，服务端包不动；升级管理端只需换管理端二进制并重启 admin 服务。

## v18（仅管理端）：登录页静态背景

- GoAdmin 登录页自带 particles.js 粒子动画背景（不停重绘），远程桌面看这个页面时编码器要不停重发画面，是卡顿的主要来源。v18 起登录页去掉粒子画布、背景改成纯色（#222d32），验证码登录不受影响。
- 管理端包 Linux 与 Windows 同步为 v18，服务端包不动；升级管理端只需换管理端二进制并重启 admin 服务。

## 回滚到旧控制台（v12，替代 GoAdmin）

- GoAdmin 接管后丢了几样旧控制台有的东西：模板自动下载、设备实时在线状态、单一账号体系、rdt_ 令牌鉴权；如需换回旧控制台，用 v12 包（`rustdesk-server-pro-windows-x86_64-v12.zip` / `rustdesk-server-pro-linux-x86_64-glibc-v12.zip`）。
- 步骤：停掉 GoAdmin admin 服务（21113）和 hbbs/hbbr → 把 hbbs/hbbr 换成 v12 的 → 重启。库是共享的（`api_v1.sqlite3`），GoAdmin 期间的用户、设备、生成记录、令牌、设置旧控制台都看得到；GoAdmin 自己的表留在库里不用，无害。
- Windows 上建议保留 v16 的 `start.ps1` + `server.env`（v12 的 `start.bat` 是极简版，没有 relay-servers），只换两个 exe；并确认 `server.env` 里填了 RELAY_SERVERS，否则外网控制器报无法连接到中继服务器。旧控制台在 `http://<服务器>:21114/console/login`（带 v7 验证码），登录账号用 users 表账号（即客户端登录账号），不是 GoAdmin 的 admin/admin。回滚后不要再用 `check-windows-v13.ps1`（它查旧控制台 404，换回后那些页是 200，会误报），改用 `check-windows-v1.ps1`。

## v19：控制台安全与体验修复（基于 v12 控制台线）

- 一次 UX 评审后的修复批，全部在旧控制台上：四张列表（设备/用户/令牌/生成客户端）的行数据渲染全面转义，OIDC 登录页参数转义，堵住存储型与反射型 XSS；吊销令牌、删除生成客户端、删除用户统一为带后果说明的确认框；删除用户现在同时吊销其名下 API Token（此前删号后令牌仍可用）；“取消管理员”补上不能对自己操作的护栏（与删除/禁用同规则）。
- 失败状态：所有列表加载失败显示“加载失败 + 重试”，不再永久停在“加载中…”；登录/注册/创建令牌/保存设置/生成客户端按钮加进行中禁用与网络错误提示。
- 设备页：切回标签或点“刷新”都会重新拉取，带“更新于”时间；新增状态列（5 分钟内有心跳显示在线，按服务端时间判断）。
- 验证码：刷新改为真正的按钮（键盘可达），图片放大；新增文字题后备——点“看不清？改用文字题”换成纯文字算术题（`GET /api/captcha?mode=text`），与图片验证码共用同一套一次性校验，供读屏用户和远控压缩画面下使用。
- 一次性令牌创建后显示在只读输入框里并带复制按钮；按钮悬停对比度修正到 WCAG AA（#2270d4）；生成客户端表新增“生成者”列。
- 升级：停服 → 替换 hbbs（hbbr 本版未改，可不换）→ 启动。数据库、server.env、密钥都不动。先在 v12 回滚确认无误后再升 v19。

## v20：模板下载改走内置下载组件（不再依赖系统 curl）

- 背景（2026-10-31 用户真机反馈）：v19 及更早版本下载官方客户端模板时调用系统 `curl`。没有 curl.exe（或不在 PATH）的 Windows 机器上，所有下载源会同时报“无法运行 curl：program not found”，模板永远下载不下来；控制台里填的 GitHub 加速镜像再好也用不上，因为每个下载源都死在启动 curl 这一步。
- 改动：`src/api.rs` 的版本解析与模板文件下载改用程序内置的下载组件（hbbs 已链接的 reqwest；Windows 走系统 TLS，Linux 走 rustls），不再调用任何外部程序。
- 行为保持不变：镜像顺序仍是自定义镜像 → 内置镜像 → 直连 GitHub；停滞检测仍在（20 秒收不到数据、或持续 15 秒低于 10 KB/s 就中止并切换下一个源）；下载完成仍校验文件大小与 MZ 文件头；控制台状态页的进度（已下载/总大小、当前源）照常显示。环境变量代理（HTTPS_PROXY 等）仍被尊重。
- 升级：停服 → 替换 hbbs（hbbr 本版未改，可不换）→ 启动。数据库、server.env、密钥都不动。
- 验证：Linux 侧在 PATH 里完全不存在 curl 的环境中实测模板下载 17/17——场景包括用户真机同款配置（自定义镜像 gh.gic.de5.net + 手填版本 1.5.0）、版本号留空自动解析、坏镜像自动切换到下一个源；下载文件 25,887,600 字节与官方发布大小一致。控制台回归套件 28/28 ×2、cargo test 20/20。Windows exe 未在本机真跑（无 Windows 环境），以真机冒烟为准。

## v21：运行文件一律落在服务端目录

- 需求（2026-10-31 用户）：服务端运行产生的所有文件都应放在服务端目录。此前各文件按进程的当前工作目录存放——从别的目录启动（双击、快捷方式、在别的路径下运行）时，数据库、密钥、日志、生成的客户端会散落到启动时所在的目录，服务端目录里反而找不到。
- 改动：hbbs 与 hbbr 启动的第一步先把工作目录锚定到可执行文件所在目录，此后所有相对路径——`db_v2.sqlite3`、`api_v1.sqlite3`、`id_ed25519` / `id_ed25519.pub`、`logs/`、`client-templates/`、`clients/`、hbbr 读取的 `.env`——都以服务端目录为根。用 start.sh / start.ps1 启动时行为不变（脚本本来就切到脚本目录，与程序同目录）。
- 迁移注意：若以前曾从别的目录启动过，旧数据文件留在那个旧目录里。换到 v21 后，先把旧目录的 `api_v1.sqlite3`、`db_v2.sqlite3`、`id_ed25519*` 搬到服务端目录再启动；若旧文件散落在多个目录，以最近还在更新的那一套为准。找不到旧文件直接启动会生成一套全新的空库和新密钥，已有客户端会全部对不上。
- 升级：停服 → hbbs、hbbr 两个都换 → 启动。server.env 不动。
- 验证：Linux 侧从一个无关目录同时启动两个程序实测 17/17——上述文件全部落在程序目录、启动目录保持全空，并实际走通注册、登录、模板目录创建与生成客户端；回归套件 28/28 ×2、模板下载（无 curl 环境）17/17、cargo test 20/20，打包用的 release 二进制同样复测通过。Windows exe 未在本机真跑（无 Windows 环境），以真机为准。

## v22：显示客户端配置（已有官方客户端直接填）

- 需求（2026-10-31 用户）：增加显示客户端配置，用于已有官方客户端直接配置。
- 内容：「生成客户端」页新增「官方客户端手动配置」卡片，显示四项——ID 服务器、中继服务器、API 服务器、密钥（Key），每项带一键复制。已经装好官方 RustDesk 客户端的用户，在客户端「设置 → 网络」里填入这四项、保存后即可登录本服务器的账号使用，不必再下载生成的安装包。
- 实现：新增 `GET /api/client-config`（任何登录用户可用；管理员尚未填写公网地址时返回未配置状态，页面显示相应提示）。配置值与生成客户端共用同一次计算（公网地址 + 启动参数端口，API 地址覆盖优先），从源头保证两边一致；实测把生成客户端的文件名解码后与接口返回值逐项比对，完全相同（含 API 地址覆盖场景）。这些值本来就编码在每个生成客户端的文件名里、随下载链接公开分发，显示出来不新增暴露面。
- 升级：停服 → 只换 hbbs（hbbr 本版未改）→ 启动。数据库、server.env、密钥都不动。
- 验证：新接口与新卡片实测 31/31（未登录 401、未配置状态、四项值、与生成客户端解码值逐项一致、普通用户可见、覆盖场景、页面标记）；控制台回归 28/28 ×2、控制台 JS 语法检查通过、cargo test 20/20；打包用的 release 二进制复测通过。Windows exe 未在本机真跑（无 Windows 环境），以真机为准。

## v23：安全加固（限速锁定、注册开关、审计日志、管理面拆分、可选 HTTPS）

- 背景（2026-10-31 用户）：21114 网页端口暴露在公网会被穷举爆破，要求做安全加固；随后追加管理面拆分与可选 HTTPS 一起做。
- 登录限速与锁定：网页登录、客户端密码登录（/api/login）、OIDC 浏览器登录、注册四个入口统一防护。失败按「账号 + 来源 IP」双维度在 10 分钟窗口内计数：同一账号失败 5 次 → 锁定 15 分钟，重复触发翻倍（30、60……上限 12 小时）；同一 IP 失败 30 次 → 按 IP 锁定（密码喷洒时，从未输错过的账号也会被挡，属预期行为）。登录成功清零该账号的失败计数。锁定期间即使密码正确也拒绝，并提示约剩多少分钟。阈值可用环境变量调整：RD_AUTH_FAIL_MAX（默认 5）、RD_AUTH_IP_FAIL_MAX（30）、RD_AUTH_WINDOW_SECS（600）、RD_AUTH_LOCK_SECS（900）、RD_AUTH_REG_MAX（10）、RD_AUTH_REG_WINDOW_SECS（3600）。计数只存内存，重启清空；审计日志是持久记录。
- 注册开关与限流：服务器设置页新增「允许开放注册」开关（默认开，与旧行为一致；公网部署建议关闭）。关闭后未登录访客注册返回 403；两个例外：库中尚无任何账号时（首次部署引导）仍可注册，管理员在登录状态下提交注册仍可建号。注册本身另有每 IP 每小时 10 次的限流（RD_AUTH_REG_MAX）。
- 认证审计日志：所有登录成功/失败、锁定触发、锁定期间的尝试、注册成功/被拦，都以 `[auth]` 前缀结构化写入 logs/hbbs.log，字段含 kind（web/client/oidc）、账号、来源 IP 与原因。例：`[auth] lockout kind=client user=admin ip=1.2.3.4 scope=account key=admin level=1 lock_secs=900`。排查谁在爆破：`grep "[auth]" logs/hbbs.log`。
- 管理面拆分：新增启动参数 `--console-listen <地址:端口>`。设置后，API 监听（如公网 21114）只提供客户端接口（登录、心跳、OIDC、设备组、下载链接），网页控制台（/console、/api/auth/*、/api/admin/*、/api/me 等）只在该地址上提供；路由器不要转发这个端口，建议填内网地址（如 192.168.1.10:21120）。start.ps1 / start.sh / server.env 已支持 CONSOLE_LISTEN 变量；服务器设置页的监听信息一行会显示控制台的独立监听地址与 HTTPS 状态。不设置该参数时行为与旧版完全一致（同一监听全功能）。
- 可选 HTTPS：新增 `--tls-cert <PEM证书>` 与 `--tls-key <PEM私钥>`（必须成对出现），启用后 API 与控制台监听都走 HTTPS，且生成客户端与配置卡片里的 API 地址自动变 https://（API 地址覆盖仍优先）。启动脚本对应 TLS_CERT、TLS_KEY 变量。重要：官方客户端会验证 API 服务器的证书，必须用受信任的证书（域名 + Let's Encrypt 等）；自签证书只适合本机联调，客户端登录会失败。没有域名时优先用「管理面拆分 + 限速锁定」，不要用自签证书硬上。证书文件缺失或不匹配时 API 服务会直接报错退出（hbbs 主服务不受影响），启用后请先确认控制台与 API 可达。
- 升级：停服 → 只换 hbbs（hbbr 本版未改）→ 启动。现有 server.env 可以不改；要启用拆分/HTTPS 时，把 CONSOLE_LISTEN、TLS_CERT、TLS_KEY 加进 server.env 再重启即可。
- 验证：v23 专项 44/44（账号锁定与到期自动解锁、IP 锁定覆盖无辜账号、注册限流、注册开关含管理员例外与审计日志落盘、拆分监听下两边路由互相隔离、HTTPS 全流程含 client-config 自动变 https），在 debug 与打包用的 release 二进制上均通过；控制台回归 28/28 ×2、客户端配置 31/31、模板下载 17/17、跨目录锚定 17/17、cargo test 25/25（新增 5 个锁定逻辑单元测试）。Windows exe 未在本机真跑（无 Windows 环境），以真机为准。

## v24：修复拆分后的下载链接 + 客户端登录页补验证码

- 背景（2026-10-31 用户真机启用 v23 拆分后报告）：① 生成客户端的下载链接打不开；② 从客户端点登录跳到的网页登录页没有验证码、还带注册入口。
- 下载链接：链接由 /dl/ 接口（在 API 监听上）提供，旧版却按「打开控制台时用的地址」拼链接。拆分后控制台在独立地址上，拼出的链接指向控制台端口——那里没有 /dl/ 接口，自然下不了。v24 起改按 API 的公网身份拼装：优先用「API 地址覆盖」（若设置了），否则用「服务器公网地址」设置 + API 监听端口（启用 TLS 时为 https）。未设置公网地址时仍按旧方式（打开控制台的地址）拼，内网单机使用不受影响。注意：已生成的旧记录不用重新生成，列表里重新复制链接即可（链接内容是实时拼的）。
- 客户端登录页（OIDC）：这是客户端「账号登录」走的网页，与控制台登录页是两套。v24 给它补上与控制台同款的验证码（图片 + 文字题后备，验证通过才查密码，失败不计入锁定计数），并移除了页面上的「先注册」链接——注册是管理面的事，不再从客户端登录页暴露。验证码接口（/api/captcha）现在两个监听都提供。
- 升级：停服 → 只换 hbbs（hbbr 未改，启动脚本与 v23 相同无需替换）→ 启动。
- 验证：专项新增 14 项——拆分模式下生成响应与列表的下载链接都指向 API 公网地址（不带控制台端口）、经 API 监听实下文件成功、双监听下验证码接口可达；OIDC 页面含验证码、无注册入口、无验证码/错验证码提交被拒、正确验证码+密码登录成功并能取回令牌。v23 专项扩至 58/58，debug 与 release 均通过；控制台回归 28/28 ×2、客户端配置 31/31、模板 17/17、锚定 17/17、cargo test 全过。Windows exe 未在本机真跑，以真机为准。

## v25：Windows 注册为系统服务 + 服务管理批处理

- 背景（2026-10-31 用户）：Windows 服务端要能注册成 Windows 服务，并有一个批处理统一管理启动/停止/重启。
- 为什么需要多一个程序：hbbs/hbbr 是普通控制台程序，不懂 Windows 服务管理器（SCM）的协议，直接用 sc.exe 注册会在启动时报 1053 错误。新增 **rdsvc.exe** 作为服务宿主（Go 编写，源码在 winservice/ 目录）：它读同一份 server.env，按与 start.ps1 完全相同的参数拉起 hbbr 与 hbbs，子进程意外退出 3 秒后自动重启，服务停止时一并停掉两个子进程；自身运行日志在 logs/rdsvc.log，子进程控制台输出在 logs/rdsvc-hbbs.exe.log / rdsvc-hbbr.exe.log。
- 新增文件（仅 Windows 包）：`rdsvc.exe`、`install-service.bat`（注册服务，开机自启，并给宿主设置崩溃自动重启策略）、`uninstall-service.bat`（停止并卸载，数据文件保留）、`service.bat`（管理入口：双击出菜单，也可带参数 start|stop|restart|status；三个脚本都会自动申请管理员权限；批处理为 GBK 编码以适配中文 Windows 的 cmd）。
- 服务名 `RustDeskPro`，显示名 RustDesk Server Pro，以 LocalSystem 账户运行。**装了服务就不要再双击 start.bat**，两边会抢占同一批端口；改配置仍是改 server.env，改完用 service.bat 重启即可。service.bat 在服务未安装时会自动按进程方式管理（启动走 start.bat、停止直接结束 hbbs/hbbr 进程），装不装服务都能用它。
- rdsvc.exe 也可直接命令行管理：`rdsvc.exe install|uninstall|start|stop|restart|status`。install 对已存在的同名服务只更新程序路径（移动服务端目录后重跑一次 install-service.bat 即可，不必先卸载）。
- 升级：停服（或关掉 start.bat 窗口）→ 替换 hbbs.exe（本版 hbbs/hbbr 与 v24 相同，实际只需把新增的 4 个文件放进服务端目录）→ install-service.bat 注册 → 之后用 service.bat 管理。
- 验证：rdsvc.exe 交叉编译通过、go vet 通过；批处理做了 GBK+CRLF 往返校验。**本机无 Windows 环境，服务注册与启停未真跑**，以真机为准——若 install 报错请把窗口文字发回。

## v26：控制台开关「允许远程修改配置」（心跳统一下发）

- 背景（2026-11-01 用户）：官方客户端默认不许控制端操作被控端的 RustDesk 客户端窗口（主界面、设置），要逐台到被控端本地勾选「允许远程修改配置」很麻烦，要求做成控制台开关统一管理。
- 实现：服务器设置新增开关 `allow_remote_config`（**默认关闭**）。心跳响应的 strategy.config_options 里在原有 hide-server-settings 之外增加 `allow-remote-config-modification: Y/N`，与「隐藏服务器设置」同一条下发管道：客户端每 15 秒心跳自动合并生效，不用重启、不用重装客户端；改开关时自动推进策略时间戳，客户端下一拍就刷新。源码核实（官方客户端 1.5.0）：客户端对下发表不设白名单，拦截远程输入读的正是这个键。
- 范围与取舍：策略是全局的，不按设备区分（本机已手动改过这一项的客户端以本机设置为准，下发的是默认值）。开启后，任何能连上某台机器的人都能改它的 RustDesk 配置（改密码、改安全选项），所以默认关闭，仅在自有设备需要远程维护其客户端时打开。
- 控制台位置：服务器设置页「允许开放注册」下方新增复选框「允许远程修改配置（通过心跳下发到所有客户端）」。
- 升级：停服 → 只换 hbbs（hbbr、启动脚本均未改）→ 启动。
- 验证：专项新增 11 项（默认下发 N、设置读写往返、开→下发 Y 且策略时间戳推进、关→回到 N），debug 与 release 均通过；全量回归：专项 69/69、控制台 28/28 ×2、客户端配置 31/31、模板 17/17、锚定 17/17、cargo test 全过、控制台 JS 语法检查通过。真机还需验一眼：打开开关后等约 15 秒，被控端设置里的「允许远程修改配置」应自动勾上，远程应能点它的客户端界面（本机无官方客户端运行时，此项以真机为准）。

## v27：多中继就近选路（按延迟近似，全球部署用）

- 背景（2026-11-02 用户）：多台中继全球部署时要用「根据延迟、最近取用」，不要无差别轮询。
- 机制（服务端单方实现，不改客户端、不改协议）：hbbs 分配中继时，用随包的城市级 IP 地理库（`geoip-city.mmdb`，GeoLite2-City）查出控制端、被控端和每台中继的经纬度，选「到两端球面距离之和」最小的中继；与最优值在 1.3 倍 + 250 km 以内的中继组成「近距离组」，组内继续轮询分流（即用户说的根据延迟轮询）。地理距离是服务端能拿到的延迟近似：实测客户端↔中继的 RTT 需要改客户端协议，官方客户端只认 hbbs 分配的地址，做不了。
- 生效条件与回退：中继 ≥2 台且地理库文件在服务端目录里（或环境变量 RELAY_GEOIP_DB 指定路径）才启用；没有库、某端 IP 查不到坐标（内网地址、anycast 地址等）、或中继本身定位不到时，自动回退原来的全量轮询，行为不劣化。选路只在健康检查的存活名单里挑（原有 3 秒探活摘除逻辑不变）。删掉/改名 mmdb 文件并重启即关闭就近选路。
- 配置：只需把多台中继照常逗号分隔写进 RELAY_SERVERS；中继地址会被拿去定位，所以填公网 IP 或能公网解析的域名。地理库是 2026-11 的 GeoLite2-City 快照，城市级精度（误差几十到几百公里，国家/大区级判断可靠）；以后想刷新，用同名文件替换即可（MaxMind 格式的城市库都兼容，如 DB-IP city lite），不刷新也能长期用，只是新分配的 IP 段可能查不到而回退轮询。
- 观察：启动日志有一行 `relay geo selection: enabled (db: ...)` 或 `no city db ... round-robin`，以此确认是否启用。
- 升级：停服 → 换 hbbs（新包里多了 geoip-city.mmdb，放服务端目录即可）→ 启动。hbbr、脚本不变。
- 验证：选路单测 7/7（含真实库查广东电信 IP 落广州、内网地址无坐标）；端到端冒烟用内置 tg 探针 12/12（中/美/澳/欧四地各归其洲中继、跨洲会话留在近距离组且最优者轮到、同城双中继互相轮询、无库时全量轮询）；全量回归 69/69、28/28 ×2、31/31、17/17、17/17、cargo test 全过。真实跨国会话的延迟改善幅度以真实部署为准（本机无全球节点可测）。

## v28：修 Windows 下设备数据库外溢 + service.bat 菜单

- 背景（2026-11-02 用户真机报告）：① 服务方式运行 hbbs 时，启动日志显示设备数据库跑到了 `C:\WINDOWS\ServiceProfiles\LocalService\AppData\Roaming\RustDesk\config\db_v2.sqlite3`——又一个文件没落在服务端目录；② service.bat 菜单按任何数字都只显示服务状态。
- 问题一根因（源码定位）：上游 peer.rs 对 **Windows 发布版**有专门分支，把 db_v2.sqlite3 拼到客户端风格的配置目录（`Config::icon_path()` 的父目录，即运行账户的 %APPDATA%\RustDesk\config）；Linux 和 Windows 调试版走的都是当前目录，所以本地测试一直没暴露（这也是它能躲过 v21 锚定专项的原因）。修复：删掉该平台分支，所有平台统一为服务端目录下的 `./db_v2.sqlite3`（进程启动时已锚定到程序目录，DB_URL 启动参数仍可覆盖）。影响：升级后第一次启动不再读旧位置的 db_v2.sqlite3，设备记录会在客户端重新注册时自动重建（约一分钟内）；想保留旧记录，先把旧路径的 db_v2.sqlite3 拷到服务端目录再启动。另外提醒：若启动日志里的 `Key:` 和控制台/现网客户端用的不一致，说明这个实例读的不是原目录的 id_ed25519（多半是解压到了新目录），先把原目录的 id_ed25519、id_ed25519.pub（及 api_v1.sqlite3）拷过来再启动，否则老客户端会验签失败。
- 问题二修复：service.bat 菜单原来用 `choice` 命令 + errorlevel 级联判断，在用户环境里表现异常（任何按键都落到 status 分支）。已换成最稳妥的写法：`set /p` 读输入 + 字符串逐个比对跳转，完全不依赖 errorlevel；并在执行前先回显一行「操作：xxx」，按错了一眼可见；输错数字会提示并重新显示菜单。三个 .bat 的定稿同时存入源码仓库 winservice/ 目录。编码仍为 GBK + CRLF，已做解码回读与逐行 diff 核对；但本机没有 Windows cmd 可执行，菜单的真机行为以下次使用为准。
- 升级：停服 → 只换 hbbs（hbbr、rdsvc 不变）+ 替换 service.bat → 启动。
- 验证：全量回归（专项 69/69、控制台 28/28 ×2、锚定 17/17、就近选路 12/12、cargo test 全过），debug 与 release 均通过。Windows 发布版的数据库路径分支已删除、逻辑全平台统一，Linux 侧验证即覆盖统一后的代码路径；真机只需确认启动日志 `DB_URL=./db_v2.sqlite3`。

## v29：同步上游中继 UUID 校验加固

- 内容（2026-11-06）：合并上游 rustdesk-server 在分叉底子 873230e 之后的 3 个新提交，核心是 44de977「Validate relay UUIDs before forwarding」：hbbs 在 RequestRelay / RelayResponse 两条路径上转发中继 UUID 前先校验——为空、超过 128 字节、或含 `[A-Za-z0-9_-]` 以外字符（含换行、空格、NUL）直接断开，不再原样转发给中继；对端确认时回送的空字段仍放行。另两个提交是给该校验补单元测试、删除上游的 CLAUDE.md。
- 与自研改动的相容性：只动 src/rendezvous_server.rs，不碰就近选路（geo_relay）、自连拒绝、控制台与 API；三方合并零冲突。正常客户端使用的 UUID / ULID 均可通过校验，已发出的客户端不受影响。
- 升级：停服 → 只换 hbbs → 启动（hbbr、rdsvc、脚本、地理库均不变）。
- 验证：全量回归（专项 69/69、控制台 28/28 ×2、模板 17/17、锚定 17/17、客户端配置 31/31、就近选路 12/12、cargo test 全过，含上游新增的 relay_uuid_validation 单测），debug 与 release 均通过。本版改动为全平台统一的协议校验，无平台专属分支、无 Windows 专属行为变更，无新增真机确认项。

## v30：同步上游加固——消息大小上限与中继/控制台修复

- 内容（2026-11-12）：合并上游 rustdesk-server 在 v29 之后的 8 个新提交（2026-11-07 至 11-08），全是加固与修 bug，无新功能、无账号改动：0415b0e 给 hbbs 与 hbbr 加消息大小上限（hbbs 默认 64 KiB、hbbr 配对前限 128 KiB，新增 `MAX_MESSAGE_KB` 配置项）；680b603 修 hbbr 双栈下 IPv4 中继黑名单匹配失效；98ffcee / 36a0996 给管理控制台回复加超时与防截断；1bd3938 给中继双向写加超时；474fbc7 限制 WebSocket 握手时长；e5c0a42 转发 WebSocket IP 时保留原端口；134008e 修配对 ID 复用后等待者过期。同时上游把 tungstenite / tokio-tungstenite 从 0.17 升到 0.29（rustdesk-org fork）。
- 与自研改动的相容性：合并时 2 处冲突，均已手工合并保留双方改动——src/rendezvous_server.rs 一处（上游的消息大小上限与我方给 1.5.0+ 客户端的问候消息合并保留）；Cargo.lock 一处（hbbs 依赖保留我方 tokio-rustls 0.23.4，tokio-tungstenite 采纳上游 0.29.0）。就近选路（geo_relay）、自连拒绝、中继 UUID 校验、控制台与 API 均不受影响。
- 升级：停服 → **同时换 hbbs 和 hbbr 两个二进制**（与 v29 只换 hbbs 不同，本批改动主要在 relay_server.rs）→ 启动（rdsvc、脚本、地理库均不变）。
- `MAX_MESSAGE_KB`（环境变量，默认 64，单位 KiB，上限 65536）：hbbs 在 TCP 与 WebSocket 监听上接受的最大消息，超大消息的连接会被断开。默认值能覆盖约 5000 个标准 9–10 位 ID 的在线状态查询；ID 特别长或数量特别多时，客户端可能看不到在线状态，可适当调大。
- 验证：全量回归（专项 69/69、控制台 28/28 ×2、模板 17/17 ×2、锚定 17/17 ×2、客户端配置 31/31 ×2、就近选路 12/12 ×2、cargo test 全过，含上游新增的中继/控制台单测），debug 与 release 均通过。本版改动为全平台统一的协议与超时加固，无新增 Windows/release 专属分支，无新增真机确认项。

## v30：新增 ARM64 Linux 包（glibc / musl 双变体）

- 内容（2026-11-13）：服务端新增 ARM64（aarch64）Linux 构建，与 v30 代码一致（合并提交 0a76ebe，无代码改动，纯交叉编译），回应 issue #2 的 ARM 软路由部署需求。两个包：
  - `rustdesk-server-pro-linux-aarch64-glibc-v30.zip`：动态链接 glibc 版，给 Debian / Ubuntu / Armbian / 树莓派 OS 等常规 ARM64 Linux 发行版；
  - `rustdesk-server-pro-linux-aarch64-musl-v30.zip`：静态链接 musl 版，单个二进制无系统库依赖，给 OpenWrt 等软路由系统（OpenWrt 没有 glibc，glibc 版跑不了）。
- 选哪个：`uname -m` 是 aarch64/arm64 的机器，先看系统——常规发行版用 glibc 版；OpenWrt / 软路由固件用 musl 版；不确定时 musl 版在两种系统上都能跑（静态链接不依赖系统库），只是体积稍大。
- 部署：与 x86_64 Linux 包用法完全一致（start.sh / server.env / systemd 服务脚本通用），解压后按上文快速开始操作；端口、配置、控制台均相同。软路由上内存与存储有限时，可删掉包里的 geoip-city.mmdb（就近选路会回退到轮询，不影响基本功能）。
- 验证：ARM64 glibc 版在 qemu-user 模拟环境通过控制台 28/28 ×2、锚定 17/17、客户端配置 31/31、模板 17/17（含自动解析 1.5.3）；musl 版通过启动与控制台 28/28 ×2 冒烟。ARM64 真机上的长期运行表现仍待实机回报。

## v31：修生成客户端「File name too long (os error 36)」

- 背景（2026-11-13，issue #1 与用户真机复现）：控制台「生成预绑定客户端」报错 `File name too long (os error 36)`，换 1.5.1 / 1.5.3 模板都一样。根因：生成客户端的配置是编码进文件名的（`rustdesk--<base64(JSON)>.exe`），文件名长度随公网地址增长——公网地址超过约 24 字符时文件名超过 255 字节，在 `clients/` 下建文件直接失败（os error 36），且原始错误原样甩回了控制台。
- 修法（src/api.rs）：生成时省略客户端能自己推导的字段。已对官方客户端 1.5.0 源码核实：`CustomServer` 四个字段都有 `#[serde(default)]`，JSON 里缺字段按空处理；空 api 按 ID 服务器端口减 2 推导，空 relay 用 hbbs 下发的中继地址（再退一步按端口加 1）。标准布局（无 API 覆盖、未开 TLS、api 端口 = rendezvous 端口 − 2、中继端口 = rendezvous 端口 + 1）下，api / relay 不再写进文件名；非标准端口、TLS、API 覆盖时字段照旧嵌入。编码文件名仍超 200 字符时回退明文短格式 `rustdesk-host=...,key=...,relay=...`（客户端同样解析；明文格式会把密钥写进文件名，只作兜底）；明文也超 255 字符时返回中文明错「公网地址太长，生成的客户端文件名超出系统限制，请使用更短的公网地址」。`GET /api/client-config` 返回值不变，仍显示完整配置；已发出的客户端不受影响。
- 升级：停服 → 只换 hbbs → 启动（hbbr、rdsvc、脚本、地理库均不变）。
- 验证：新增 5 个单元测试（省略字段、40 字符域名仍走编码格式、超长回退明文、超长中文报错、省略判定），cargo test 47/47（debug 与 release）；端到端 19/19（debug 与 release：40 字符域名生成成功，文件名 161 字符且解码后 host/key 正确、api/relay 已省略；85 字符域名走明文兜底；240 字符域名报中文明错）。全量回归（专项 69/69 ×2、控制台 28/28 ×2、模板 17/17 ×2、锚定 17/17 ×2、客户端配置 31/31 ×2、就近选路 12/12 ×2），debug 与 release 均通过。本版改动为全平台统一的文件名生成逻辑，无新增 Windows/release 专属分支，无新增真机确认项。

## v32：在线一键更新 + API 调用说明

- 功能（2026-11-13）：控制台服务器设置页新增「服务端版本更新」区，显示当前版本 v32（附带上游基底 1.1.17）与运行平台（windows-x86_64 / linux-x86_64-glibc / linux-aarch64-glibc / linux-aarch64-musl 四者之一）；「检测更新」按钮查本仓库（wirh/rustdesk-server-pro）的最新 release；有新版时在「更新」按钮上方展示完整更新日志面板（可滚动，点更新前能看清新版改了什么）；下载中显示进度与来源；暂存完成后显示「已暂存 vX，重启后生效」+「立即重启」按钮；重启后页面自动等待、服务端恢复响应后显示新版本。版本号比较去掉前导 `v` 后按整数比（v32 > v31）；非数字 tag 视为无更新并原样展示。
- 流程：检测 → 下载暂存 → 重启替换。下载只从本仓库的 release 取对应平台的安装包：版本号严格校验 `^v[0-9]+$`，绝不接受调用方提供的任意 URL；下载校验（不小于 1MB + ZIP 魔数 `PK\x03\x04`）；解压出 hbbs 与 hbbr 两个二进制，暂存为 exe 目录下的 `hbbs.new` / `hbbr.new`（Windows 为 `hbbs.exe.new` / `hbbr.exe.new`），同时写 `update/staged.json` 记录版本；失败时清理临时文件，不留半截二进制。下载在后台线程进行，可用 `GET /api/admin/update/status` 轮询进度（详见下文 API 一节）。
- 检测重试：先直连 api.github.com 查 `releases/latest`，失败时依次用镜像链重试同一 API 请求；镜像 API 也失败时，再用镜像链请求 `releases/latest` 页面，从重定向 Location 里取版本号（这条路只有版本号、拿不到更新日志，返回里标记 `notes_unavailable`，界面显示「更新日志暂不可用」）。全部路径都失败才返回检测失败（502），并说明已尝试直连与加速。
- 下载兜底：API 查不到发布信息时（例如 GitHub 未登录 API 的 60 次/小时共享限流），改用确定性的 `releases/download/{version}/{包名}` 直链走镜像链下载；该 URL 由校验过的版本号与编译期平台拼成，不接受调用方输入，防 SSRF 的保证不变。不存在的版本在所有下载源上都 404，最终仍进入错误状态。
- 重启替换：替换必须发生在启动器执行二进制之前（Windows 下运行中的 exe 被锁死，hbbs 启动时换不了自己；Linux 下 hbbs 启动时替换会导致当次运行的仍是旧二进制）。rdsvc（Windows 服务宿主）、start.sh、start.ps1、systemd（install-service.sh 生成的 unit 加了 `ExecStartPre`）都在启动子进程前检查同目录的 `.new` 文件并覆盖正式文件，成功后删除 `update/staged.json`；替换失败只记日志、不阻塞启动（下次重启再试）。直接运行 hbbs（无启动器托管）时 `.new` 不会自动替换：需手动把 `hbbs.new` 改名为 `hbbs`（`hbbr.new` 改名为 `hbbr`）、删除 `update/staged.json` 后再启动，控制台文案有此说明。
- 镜像设置：更新下载与检测复用服务器设置页已有的 `github_mirror` 设置和模板下载的同一条内置镜像链，没有第二个镜像设置。
- 新增依赖：解压用 `zip` 8.6.0（仅 `deflate-flate2` 特性），Cargo.lock 的改动只涉及该 crate 及其传递依赖（flate2、miniz_oxide、crc32fast 的版本统一）。
- 升级：停服 → 换 hbbs（hbbr 无改动，不用换）→ **Windows 下同时换 rdsvc.exe，Linux 下同步替换 start.sh / install-service.sh（Windows 下换 start.ps1）**——替换逻辑在启动器里，旧版启动器没有 `.new` 检查，暂存后重启不会生效 → 启动。API 调用说明见下节。
- 验证：cargo test 56/56（debug 与 release，各含 9 个新增单测：版本号解析/比较、平台映射、版本参数校验、镜像候选、重定向取 tag、release 解析、解压往返）；端到端 55/55（debug 与 release：真实 GitHub 检测且更新日志与 release body 逐字一致、v31 真实下载暂存、监督脚本模拟重启替换后文件哈希与暂存一致、服务恢复响应）；全量回归（专项 69/69 ×2、控制台 28/28 ×2、模板 17/17 ×2、锚定 17/17 ×2、客户端配置 31/31 ×2、就近选路 12/12 ×2）。真机边界：rdsvc 与 start.ps1 的替换逻辑只在交叉编译 + go vet + 编码检查层面验证过，需在真实 Windows 服务环境确认；systemd 的 `ExecStartPre` 需在真实 Ubuntu 上确认。

## v33：生成客户端页增加其他平台官方下载链接

- 功能（2026-11-13）：控制台「生成客户端」页新增「其他平台下载」区，列出最新官方客户端的 macOS（Intel / Apple 芯片 dmg）、Linux（x86_64 与 ARM64 的 .deb 与 AppImage）、Android（arm64 / x86_64 / armv7 / 通用 apk）安装包，每个平台一行：官方 GitHub 直链 + 加速下载链接；另有 iOS 的 App Store 直链（iOS 没有 GitHub 安装包，不走镜像）。其他平台用户装好官方客户端后，按上方「官方客户端手动配置」卡片填写服务器地址与密钥即可使用。本版只做下载链接，不做多平台预绑定生成（Windows 生成客户端不变）。
- 一键复制配置（2026-11-13）：「官方客户端手动配置」卡片新增「一键复制配置」按钮，复制整份配置为一个字符串，粘贴到官方客户端「设置 → 网络」的粘贴（导入）图标处即可一次填入四项，无需逐项填写。字符串格式与官方客户端自身的导出完全一致（`ServerConfig.encode()`，已对 1.5.0 与 1.5.3 源码核实）：`{"host","relay","api","key"}` 四字段 JSON → base64url（无填充）→ 整体反转；也是生成客户端文件名里用的同一段编码，与付费 Pro 控制台的 Config 字符串同一格式。`GET /api/client-config` 新增 `share` 字段返回该字符串。
- 接口：新增 `GET /api/client-downloads`（任何登录用户可用，与 `/api/client-config` 同级），返回 `{"ok":true,"version":"1.5.3","platforms":[{"id","name","file","url","mirrors":[...]}]}`。版本号与文件名来自 api.github.com 上 rustdesk/rustdesk 的 `releases/latest`：文件名是 release 里真实的 asset 名（按 `rustdesk-{版本}-` 前缀 + 已知后缀匹配，改名或缺失的 asset 直接让该平台不出列表，绝不拼一个没验证过的链接）；`url` 取 asset 自身的 `browser_download_url`。查不到版本时返回 502 + 中文明错，不编造版本号。
- 缓存与加速：上游 release 数据内存缓存 1 小时（GitHub 未登录 API 限流 60 次/小时/共享 IP，生成页每次打开都查会很快撞上限）；加速链接不进缓存，每次请求按当前 `github_mirror` 设置现算——管理员改镜像后立即生效。加速用模板下载与在线更新的同一条镜像链（管理员自定义镜像优先，其次内置镜像，最后直连），没有第二套镜像逻辑。检测复用模板下载的最新版解析（镜像优先、直连兜底）。
- 升级：停服 → 只换 hbbs → 启动（hbbr、rdsvc、脚本、地理库均不变）。
- 验证：新增 7 个单元测试（下载链接 4 个：按真实 1.5.3 asset 名解析 10 个平台并排除 Windows 安装包/rpm/flatpak/sciter/wayland 变体、缺 tag 报错、镜像 URL 生成、缓存新鲜度；一键复制 3 个：分享字符串与生成客户端文件名同一编码、与官方客户端导出逐字节一致、四字段始终齐全），cargo test 63/63（debug 与 release）；下载端到端 50/50（debug 与 release：版本号与 `gh api` 的最新版一致、平台文件与真实 asset 名逐项核对、官方 URL 格式、镜像含已配置的自定义前缀、匿名 401、普通用户可访问、控制台页面标记）；客户端配置端到端 38/38 ×2（含分享字符串按客户端解码复刻反向解析、四字段与接口值一致、普通用户同值、控制台页面标记）。全量回归（专项 69/69 ×2、控制台 28/28 ×2、模板 17/17 ×2、锚定 17/17 ×2、就近选路 12/12 ×2），debug 与 release 均通过。本版改动为全平台统一逻辑，无新增 Windows/release 专属分支，无新增真机确认项。

## v34：控制台中英切换 + 英文部署文档 + 下载镜像加速复选框

- 语言切换（2026-11-14）：控制台右上角新增「中文 / English」切换，登录/注册页与客户端登录页同样有切换按钮。默认中文；切换立即生效，刷新/换页后仍保持（选择存在浏览器 localStorage 里，键 `rdp-lang`）。翻译覆盖控制台全部界面文案（登录、注册、概览、设备、用户管理、API Token、生成客户端、服务器设置各页的标签、按钮、提示语与确认框）；API 返回的错误字符串不翻译（客户端依赖原文，如 OIDC 的 "No authed oidc is found"）。实现是 JS 词典（src/api_pages.rs 内嵌，约 135 个键；页面源码里中文仍为默认文案，英文在运行时替换，所以页面标记检查不受影响）。
- 英文部署文档（2026-11-14）：新增 docs/DEPLOY-en.md，DEPLOY.md 的完整英文翻译（各版本章节、端口表、API 调用说明、ARM 一节齐全）；根 README 顶部加了英文文档链接；安装包里同时带 README.md（中文）与 README-en.md（英文）。
- 镜像加速复选框（2026-11-14）：「其他平台下载」卡片改成默认只显示官方直链；卡片上方新增「镜像加速」复选框（英文界面显示 "Use mirror"），勾选后各平台链接切换为镜像加速地址，取消切回直链。镜像地址仍用模板下载/在线更新的同一条镜像链（管理员 `github_mirror` 设置优先，其次内置镜像），没有第二套逻辑；复选框状态存在 localStorage 里（键 `rdp-dl-mirror`），刷新后保持。iOS 只有 App Store 直链，没有镜像，不受复选框影响。`GET /api/client-downloads` 返回结构不变（每平台本来就同时带 `url` 与 `mirrors`）。
- 升级：停服 → 只换 hbbs → 启动（hbbr、rdsvc、脚本、地理库均不变）。
- 验证：cargo test 63/63（debug 与 release；无新增服务端逻辑，单测只改版本号断言）；浏览器端到端 43/43（debug 与 release，Playwright 真实浏览器：默认中文、切换英文后各页文案与动态表格均变英文、刷新后保持英文、切回中文正常；镜像复选框默认未勾选且链接为直链、勾选后变镜像、iOS 不变、取消切回、刷新后保持勾选状态）；下载端到端 50/50 ×2、客户端配置端到端 38/38 ×2；全量回归（专项 69/69 ×2、控制台 28/28 ×2、模板 17/17 ×2、锚定 17/17 ×2、就近选路 12/12 ×2），debug 与 release 均通过。本版改动为全平台统一逻辑，无新增 Windows/release 专属分支；语言切换与复选框在真实浏览器上的表现已由 Playwright 套件覆盖，无新增真机确认项。

## v35：必须登录开关（控制台复选框）

- 功能（2026-11-15，GitHub issue #3）：服务器设置页新增「必须登录」复选框（英文界面 \"Require login\"），默认关闭。开启后，所有客户端必须登录后才能发起连接：控制端的登录 token 为空或在 `client_tokens` 表里查不到时，打洞阶段直接拒绝，客户端显示「未登录，无法连接 / The connection is not allowed. You have not logged in.」；已登录客户端的合法 token 正常放行。开关存在 settings 表（键 `must_login`），改动即时生效，不用重启。`client_tokens` 表没有过期字段，token 只在登出或删号时删除，所以「过期」等同于「查不到」，拒绝条件写「空 token / 查不到」即可覆盖。
- 无绕过：拒绝分支放在 `handle_punch_hole_request`（自连检查之后），TCP、UDP 与 WebSocket 打洞都走同一个处理，没有哪条路能绕过；同时堵上了中继请求的口子——被控端收到转发的 `RequestRelay` 就会去中继配对（不要求先有打洞），所以 `RequestRelay` 也按同一规则检查 token，未登录时回 `RelayResponse{refuse_reason}` 拒绝，不转发给被控端。hbbr 不用改：中继配对只在被控端收到 hbbs 转发的打洞/中继请求后才发生，打洞阶段拦住就覆盖了常规中继流程。开关与 v23 防爆破锁定正交，可叠加生效。
- 升级：停服 → 只换 hbbs → 启动（hbbr、rdsvc、脚本、地理库均不变）。
- 验证：新增 4 个单元测试（拒绝决策矩阵：开关开/关 × token 空/查不到/有效；登录上下文：缺数据库时 fail open、开关开时解析 token、开关关或缺失时不启用），cargo test 67/67（debug 与 release）；新增必须登录端到端 21/21（debug 与 release：真实 PunchHoleRequest / RequestRelay 协议消息驱动 TCP 21116 与 WebSocket 21118，覆盖开关关时空/未知 token 放行、开关开时空/未知 token 被拒、合法 token 放行、自连仍先被拒、中继请求同规则、WS 同规则、开关关后立即恢复）；全量回归（专项 69/69 ×2、控制台 28/28 ×2、模板 17/17 ×2、锚定 17/17 ×2、就近选路 12/12 ×2、下载 50/50 ×2、配置 38/38 ×2、语言切换 43/43 ×2），debug 与 release 均通过。本版改动为全平台统一逻辑，无新增 Windows/release 专属分支；UDP 打洞与 TCP/WS 共用同一处理（源码核实），沙箱拦 UDP 故未在端到端里直接驱动。无新增真机确认项。

## v36：中继节点管理（注册 + 配置下发 + 状态查看）

- 背景：hbbr 本身从不连接 hbbs，hbbs 只知道中继地址与存活（3 秒 TCP 探测），所以控制台一直看不到中继节点、也管不了它们。v36 在中继机器上加一个常驻代理 relay-agent（Go，`relayagent/`，与 hbbr 同目录运行），由它负责注册、心跳、收配置、监督本地 hbbr 进程。hbbs 侧新增 `relay_nodes` 与 `relay_reg_tokens` 两张表与管理接口；hbbr 本体不用改（代理通过 hbbr 自带的回环控制台读写限速与会话数）。
- 注册流程（2026-11-15）：管理员在控制台「设备」页点「生成注册链接」（可填备注，有效期 1–168 小时，默认 24 小时；需要先在服务器设置里配好 `public_host`，否则生成时报与生成客户端相同的错误）。链接形如 `{API 地址}/relay/register?token=rrt_…&key=…`，只在生成时显示一次，页面同时给出中继机器上要运行的命令（`relay-agent register --link <链接>`）。代理分两步注册：先拿 token 标识换一个一次性质询 nonce，再用链接里的密钥做 HMAC 证明身份；服务端校验通过后把节点记入 `relay_nodes`，并把服务器密钥对（id_ed25519 + id_ed25519.pub）与初始配置加密下发。代理收到后落盘（Linux 上私钥 0600），写好 agent.json 与 relay.env，然后 `relay-agent run`（或注册为 Windows 服务 `RustDeskRelayAgent`）即开始监督 hbbr 与心跳。链接一次性、用完即废、过期即废；「待使用的注册链接」列表可吊销未用链接。注册链接页面（GET /relay/register）可直接打开查看链接是否仍有效。
- 密钥与加密传输：注册链接里带一个 32 字节随机对称密钥（注册密钥），它是双方共享的唯一密钥：服务端用它加密下发密钥对与全部配置（AES-256-GCM，每条消息独立随机 nonce），代理本地解密后才应用。**密钥与注册 token 绝不在任何请求/响应里出现**：注册认证是 HMAC 质询-响应（服务端发 nonce，代理用密钥签名证明身份），心跳是加密后签名（encrypt-then-MAC：先用 AES-256-GCM 加密状态，再用 HMAC-SHA256 签 node_id + 时间戳 + nonce + 密文，服务端验签并记 nonce 防重放，时间戳偏差超过 5 分钟拒绝）。威胁模型：这套机制防被动窃听（明文 HTTP 下抓包也拿不到密钥、token 与配置）；主动中间人只能阻断或篡改（篡改会被 AEAD/HMAC 校验发现：代理解密失败时拒绝应用并记日志告警，服务端验签失败时拒绝请求），拿不到密钥也伪造不了配置。剩余的信任锚是注册链接本身——链接泄露后，拿到链接的人可在有效期内注册并领走服务器密钥对，所以链接要当作凭证妥善保管、用完即删、尽快使用；节点删除即吊销（服务端删密钥记录，之后的心跳验签失败，代理停止心跳但 hbbr 继续运行）。v1 不做密钥轮换（一次性链接 + 删除即吊销已覆盖主要场景；以后如需轮换，可在心跳包里用旧密钥加密下发新密钥）。有 TLS 部署时建议走 TLS（连篡改/阻断的窗口也一并收窄）。服务端以 `-k -` 方式启动 hbbr（从下发的密钥文件读密钥对，与 hbbs 默认行为一致）；如果密钥文件丢失，代理拒绝启动 hbbr（fail closed，不会放一个无密钥中继出去）。
- 配置下发：控制台节点详情里可改监听端口与三项限速（LIMIT_SPEED / TOTAL_BANDWIDTH / SINGLE_BANDWIDTH，单位 Mb/s；留空表示 hbbr 默认值 32 / 1024 / 128；0 不是可用值，服务端会拒绝——hbbr 会忽略 0），也可远程重启 hbbr。配置存在服务端，随下一次心跳响应加密下发（20 秒内生效）；端口变化时代理重启 hbbr（该节点上的中继会话会中断），只改限速时通过回环控制台即时生效、不重启；代理在心跳里回执已生效的配置版本，控制台详情显示「已生效版本 / 服务端版本」。管理接口（均需管理员登录）：`POST /api/admin/relays/reg-token`、`GET /api/admin/relays/reg-tokens`、`DELETE /api/admin/relays/reg-tokens/:id`、`GET /api/admin/relays`、`GET /api/admin/relays/:id`、`DELETE /api/admin/relays/:id`、`POST /api/admin/relays/:id/config`、`POST /api/admin/relays/:id/restart`；代理侧接口（无会话认证，HMAC + AEAD）：`POST /api/relay/challenge`、`POST /api/relay/register`、`POST /api/relay/heartbeat`。
- 设备列表与状态：管理员在「设备」页看到中继子表（带「中继」徽标），点节点名进详情。详情含在线/离线（按最后心跳 90 秒判）、地址、上次来源 IP、代理与 hbbr 版本、hbbr 运行状态与启动时间、最后心跳、当前生效配置、密钥已交付状态、配置版本与待执行命令。会话数与吞吐来自 hbbr 回环控制台的 `u` 输出（真实配对会话数与实时速率汇总，不是估算；hbbr 每 3 秒才刷新一次会话统计，所以刚建立不到 3 秒的会话可能暂时显示 0）。普通用户在设备列表里看不到中继节点（`/api/devices` 的 `relays` 数组只对管理员返回）。
- 管理面与轮询列表的区别（重要）：注册成功只建立管理关系，**不会**把节点自动加入中继轮询列表。轮询列表仍只由 hbbs 启动参数 `--relay-servers`（server.env 里的 RELAY_SERVERS）决定；要让新中继真正接流量，仍需在 server.env 里加地址并重启 hbbs。v36 没有改轮询逻辑。
- 中继安装（单独的中继包，不含 hbbs）：每个平台一个 `rustdesk-server-pro-relay-*` 包，内含 hbbr、relay-agent 与安装脚本。Linux：解压后运行 `install-relay.sh`（装 systemd 服务 `rustdesk-relay-agent`，服务拉起代理、代理监督 hbbr；**被管中继上只由代理启动 hbbr，不要再用 systemd / rdsvc 直接拉 hbbr**，否则两个进程抢端口），然后在包目录里运行 `relay-agent register --link <链接>`，再 `systemctl start rustdesk-relay-agent`。Windows：解压后运行 `install-service.bat`（注册 `RustDeskRelayAgent` 服务），在包目录里运行 `relay-agent.exe register --link <链接>`，再 `service.bat start`（或 `relay-agent.exe start`）。ARM 软路由用 linux-aarch64-musl 中继包（hbbr 与代理都是静态链接）。中继机器只需放通中继端口（默认 21117 TCP，WebSocket 在端口+2）；代理连服务端走 API 端口（默认 21114），中继在 NAT 后面也能用（心跳是代理主动发起的出站连接）。
- 升级：停服 → 只换 hbbs → 启动（hbbr、rdsvc、脚本、地理库均不变）。中继包是新增的部署物，首次部署按上一段安装；已注册的中继节点在服务端升级后不受影响（节点记录与密钥都在数据库里，心跳继续）。
- 验证：新增 8 个 Rust 单元测试（token_id 已知向量、注册/HMAC 消息逐字构造、HMAC-SHA256 已知向量与篡改拒绝、AES-256-GCM 往返与篡改/错密钥拒绝、配置解析校验），cargo test 75/75（debug 与 release；lib 60 + hbbr 14 + 集成 1）；Go 代理单测 28/28（`go test`，`-race` 下同样干净；`go vet` Linux/Windows 双目标干净）；新增中继端到端 68/68（debug 与 release：Python 独立实现的注册/心跳协议与 Rust 服务端互通——注册包解密后密钥与服务端逐字节一致、响应体不含明文密钥材料、重放/篡改/过期时间戳均被拒；真实 relay-agent 走完注册 → 密钥文件 0600 落盘 → 心跳 → 下发改端口（hbbr 重启生效）→ 只改限速即时生效不重启 → 远程重启 → 吊销后停心跳但 hbbr 继续服务；真实 WebSocket 中继配对双向转发通过，错密钥被拒；注册链接复用/过期/吊销均被拒）；全量回归（控制台 28/28 ×2、模板 17/17、锚定 17/17、配置 38/38、专项 69/69、就近选路 12/12、下载 50/50、语言切换 43/43、必须登录 21/21），debug 与 release 均通过。本版 Rust 改动为全平台统一逻辑，无新增 Windows/release 专属分支；Go 代理的 Windows 服务与 ACL 代码只在交叉编译层面验证（见真机确认项）。控制台 JS 经 node --check 语法校验，i18n 词典中英 287/287 对齐。
- 真机确认项：Windows 代理的服务注册/启停与私钥 ACL 收紧只到交叉编译通过，未在真实 Windows 上跑过（ACL 失败时只记日志、不中断注册，最坏情况是私钥停留在默认 ACL）；ARM 中继包与 ARM 服务端包一样只在 qemu 下验证，未在真实 ARM 设备上跑过。Linux 代理的注册、心跳、配置下发、重启、吊销与真实中继配对已在端到端里用真实二进制跑通。

## v37：主部署中继也校验密钥（hbbr 加 -k -）

- 背景（2026-11-15）：v36 发现主部署的中继一直在"裸奔"——start.sh、start.ps1 与 rdsvc 启动 hbbr 时只传了 `-p`，没传 `-k`，所以中继接受任意 licence_key（上游默认行为，v36 之前就存在，不是 v36 引入的）。v36 的中继代理已经用 `hbbr -p <端口> -k -` 启动 hbbr；本版把主部署的三个启动器也改成同样做法。`-k -` 的语义（已对源码核实）：hbbr 从服务端目录的 `id_ed25519` 读密钥对、取其公钥作为中继密钥；客户端发来的 `RequestRelay` 里 licence_key 与之不符时，中继直接断开连接、不配对。注意：不带 `-k` 时 hbbr 不校验任何密钥。
- 全新安装的顺序保护（重要）：`-k -` 有个坑——`id_ed25519` 不存在时，hbbr 会自己生成一套新密钥对。如果 hbbr 比 hbbs 先启动并造了密钥，hbbs 启动时就会沿用 hbbr 那套（谁先创建谁赢），结果是整个部署的密钥变成 hbbr 生成的那套；更糟的是两人同时抢创建时输的一方会启动失败。所以三个启动器现在都是"hbbs 先启动，hbbr 等密钥文件出现后再启动"：start.sh 把 hbbs 放后台先跑，轮询 `id_ed25519`（最长 30 秒，可用环境变量 `KEY_WAIT_SECS` 覆盖），出现后才启动 `hbbr -p <端口> -k -`，最后 `wait` hbbs；start.ps1 同样先起 hbbs（共享本窗口输出）、等密钥文件、再起 hbbr（独立窗口）、最后等 hbbs 退出；rdsvc 把 hbbs 子进程先拉起，hbbr 子进程每次启动前都等密钥文件（30 秒超时后记日志、10 秒后重试，服务停止时立即放弃等待）。密钥文件在超时内始终没出现时，hbbr 不启动、报明错（中继停着但不至于用错密钥服务）；hbbs 中途退出时也一样直接报错退出。systemd 服务走 start.sh、自动覆盖；service.bat / install-service.bat 只调 rdsvc、不直接启动 hbbr，不用改；中继包走 relay-agent（v36 已用 `-k -`），不用改。
- 升级：停服 → 换 hbbs、hbbr（二进制本身无行为改动，但与新版启动器配套）、rdsvc.exe（Windows 服务部署）与 start.sh / start.ps1 → 启动。**升级前确认所有客户端的密钥与服务器一致**：加上 `-k -` 后，密钥不一致的客户端走中继会被拒（直连不受影响——直连不经过 hbbr）。已在用的部署升级后密钥文件早已存在，不存在顺序问题；全新安装按上一段的顺序保护自动处理。
- 验证：新增 v37 端到端 28/28（debug 与 release：全新空目录经 start.sh 启动后 hbbs 与 hbbr 共用同一密钥对——公钥文件、hbbr 日志里的 Key、/api/client-config 的 key 三处一致；正确密钥的客户端经 WebSocket 中继真实配对双向转发通过、错密钥被拒；hbbs 立即退出时 start.sh 快速报错、hbbr 不启动且不造密钥；hbbs 始终不造密钥时 2 秒超时报错并杀掉卡住的 hbbs、hbbr 不启动；三个启动器的 -k - 与等密钥逻辑、start.ps1 的 UTF-8 BOM、.bat/中继安装脚本不直接启动 hbbr 均经检查）。cargo test 75/75（debug 与 release；无新增服务端逻辑，单测只改版本号断言）；Go rdsvc `go vet` 通过、Windows 交叉编译通过（PE32+）；start.ps1 经 pwsh 语法检查通过。全量回归（控制台 28/28 ×2、模板 17/17 ×2、锚定 17/17 ×2、配置 38/38 ×2、专项 69/69 ×2、就近选路 12/12 ×2、下载 50/50 ×2、语言切换 43/43 ×2、必须登录 21/21 ×2、中继 68/68 ×2），debug 与 release 均通过。本版 Rust 改动只有版本号字符串，无新增 Windows/release 专属分支。
- 真机确认项：rdsvc 与 start.ps1 的新顺序（hbbs 先起、等密钥、再起 hbbr）只在交叉编译与语法层面验证，未在真实 Windows 上跑过——请在真实 Windows 服务端上确认：服务启动后 hbbs 与 hbbr 都起来了、logs/hbbr.log 里有 `Key: <与 id_ed25519.pub 一致的公钥>`、客户端经中继能连、错密钥的客户端被拒；以及 service.bat 的启动/停止/重启仍正常。

## v38：21116 控制通道 KX 加密（secure_tcp，KX v0）

- 背景（2026-11-16，GitHub issue #4）：21116 TCP 控制通道一直是明文——登录 token、打洞与中继请求都在裸奔，路径中间人可嗅探。issue 里说的"新客户端登录走不通"在我们这个 fork 上不成立：服务端之前先发一条空问候消息，≥1.5.0 客户端收到非 KeyExchange 首消息后按 pre-exchange 服务端回退走明文，所以 1.5.3 登录一直是通的；真正的缺口是安全不是兼容。本版实现 KX v0：TCP 21116 上客户端与服务端先做密钥交换，之后的消息全部对称加密。握手逻辑移植自 sctgdesk-server（AGPL-3.0，与本项目同许可证）。
- 握手过程：服务端先说话（phase 1）：把本进程的 X25519 公钥用持久的 Ed25519 服务器密钥（id_ed25519）签名后发给客户端；客户端用配置里的服务器公钥验签，验签通过后生成临时 X25519 密钥对与随机对称密钥，把对称密钥密封给服务端公钥、随临时公钥一起发回（phase 2）；服务端用 X25519 私钥解开密封拿到对称密钥，此后双向消息都用 secretbox + 序号 nonce 加密。KX v0 只用一个对称密钥管双向；v1 的 signed_params 域绑定本版不做。
- 密钥对管理：X25519 密钥对每次启动时新生成、不落盘——公钥是靠持久 Ed25519 签名逐连接认证的，持久化没有额外的信任价值；重启即换也限制了单个密钥对的暴露窗口。Ed25519 密钥对仍是 id_ed25519（不变）。没有签名密钥时（`-k` 只传了公钥的极端情况）不发 phase 1，改发空问候，行为与 v38 之前一致。
- 兼容性：新客户端（支持 secure_tcp 的版本，包括 1.5.0/1.5.3）自动走加密，无需改配置；老客户端（无 KX）收到 phase 1 消息后在消息循环里忽略，随后照常发明文消息，服务端没有协商到密钥就按明文处理，连接与登录不受影响。未配置服务器密钥的客户端会用官方默认公钥验签，验签失败导致 TCP 控制通道建不起来（fail-closed，符合设计）——正确做法是给客户端配上本服务器的密钥（生成客户端已预绑定）。WebSocket 21118 不变（客户端的 secure_tcp 跳过 WS；WS 上收到 KeyExchange 属于协议违规，服务端直接断开）。UDP 打洞不变（无 KX；打洞路径不传凭证）。v35 的必须登录不受影响（加密通道上的打洞/中继请求同样过 token 检查）。WebRTC 的 SDP/ICE 路径之前因 KX 未完成而被客户端主动丢弃，现在 KX 完成，这些路径会正常走加密通道发送——这是预期内的行为改善。另外注意：TCP 连接仍是一次请求一条连接（上游原有行为，KX 未改变）：除打洞/中继请求外，服务端处理完一条 TCP 消息后即关闭连接。
- 控制台 URL 状态：控制台 6 个区与中继节点详情写进地址栏 hash（`#/devices`、`#/devices/relay-3` 这类形式）：点导航按钮时 hash 跟着变，刷新或收藏后打开会回到同一个区而不是总回概览，浏览器前进/后退同步生效；非法 hash 与非管理员打开管理区时回退到概览；中继详情深链在设备列表加载完成后恢复；只改控制台前端状态，不动布局与接口；更新重启后的回设置页逻辑不变。
- 设备列表「最近使用的中继」列：每次中继会话实际用的中继记到会话两端设备的 `last_relay`（附时间戳 `last_relay_at`），控制台设备表新增一列展示，未经过中继会话的设备显示 `-`。注意语义是"最近用过"：官方客户端在心跳、sysinfo、注册里都不上报自己的中继设置，服务端拿不到"配置的中继"；本列取的是被控端回的 RelayResponse 里的中继地址（打洞发起的中继会话）与控制端 RequestRelay 里的中继地址（中继请求发起的会话），即会话实际走的中继；改配置后要下一次中继会话才会在本列体现。记录的地址是被控端解析出的中继（局域网改写之前的值）；控制端的归属按登录 token 或地址唯一匹配确定，同一公网 IP 下有多台设备时不猜，只记被控端；写库失败只记日志，不影响中继流程。旧数据库升级时启动自动加列。
- 升级：停服 → 只换 hbbs → 启动（hbbr、rdsvc、脚本、地理库均不变）。
- 验证：新增 11 个单元测试（KX 6 个：客户端密封的密钥服务端可解开、篡改/错对端密钥被拒、phase 1 签名验签与错公钥拒绝、生成的 X25519 公钥最高位恒清、v0 双向流互通与序号推进；中继记录 2 个：两端更新/无行不补/缺库放行；地址唯一匹配 3 个：唯一命中/多设备不猜/零端口忽略），cargo test 86/86（debug 与 release；lib 71 + hbbr 14 + 集成 1）；新增 KX 端到端 25/25（debug 与 release 各两遍：Python 客户端按 1.5.0 客户端的 key_exchange 实现——phase 1 形状与签名验签、加密往返、线上传的是密文（原始帧与明文编码不同且解析不出对应消息）、篡改/畸形/错密钥握手均被断开、老客户端明文路径仍通、加密通道上的必须登录拒绝与放行、WS 不变且 WS 上的 KeyExchange 被断开、重启后换新临时密钥对仍能握手）；新增 hash 路由端到端 38/38（debug 与 release 各两遍：点区写 hash、刷新保持、前进/后退生效、中继详情深链恢复、非法 hash 与非管理员回退概览、最近中继列中英文表头与有值/无值渲染）；新增最近中继端到端 25/25（debug 与 release 各两遍：真实 RelayResponse/RequestRelay 协议消息驱动——被控端与控制端（登录 token）两端记录、新会话覆盖旧值、空值/无名设备不记录不补行、旧数据库启动自动加列且重启无碍）；全量回归 52 次运行全绿（控制台 28/28 ×2、模板 17/17 ×2、锚定 17/17 ×2、配置 38/38 ×2、专项 69/69 ×2、就近选路 12/12 ×2、下载 50/50 ×2、语言切换 43/43 ×2、必须登录 21/21 ×2、中继 68/68 ×2、KX 25/25 ×2、hash 路由 38/38 ×2、最近中继 25/25 ×2），debug 与 release 均通过；ARM 两个变体 qemu 控制台套件 28/28 ×2。本版 Rust 改动集中在 src/rendezvous_server.rs、src/peer.rs、src/api.rs（全平台统一逻辑）与控制台前端 src/api_pages.rs，无新增 Windows/release 专属分支；hbbr 不动。
- 真机确认项：真实客户端（1.5.3）连上后，客户端日志里有 "Connection secured" 即表示加密通道已建立；服务端侧的 "KeyExchange symmetric key set" 是 debug 级日志，默认 info 级别下不显示，需 RUST_LOG=debug 才可见。另请确认老版本客户端仍能正常连接登录。最近使用的中继列：真实中继会话后，控制台设备列表对应设备行显示实际走的中继；同一公网 IP 下有多台设备时，控制端一侧可能无值（不猜，属预期行为）。

## v39：用户管理里管理员直接建号（Fixes #6）

- 背景（2026-11-17，GitHub issue #6）：关闭注册（allow_register=0）后管理员建号是死循环——「注册」按钮只在登录页，须退出登录才看得到，退出后注册又被拒绝；用新标签页直接打开注册 URL 能建号，但建号成功后服务端给新用户签发会话，把管理员的登录态顶掉。控制台用户管理也没有建号入口。本版补上管理员建号入口。
- 新增接口（控制台管理面，管理员专用）：`POST /api/admin/users`，请求体 `{"username": "...", "password": "..."}`。用户名规则与注册一致（3–32 位字母、数字、`_-.)；密码至少 6 位（与重置密码规则一致）；用户名已存在返回 400。成功只返回 `{"ok": true}`，不给新用户签发会话——管理员保持登录。非管理员调用返回 403，未登录返回 401。管理面拆分后只能走 console 地址，API 地址上是 404。每次建号记一条 `[auth] user-created ... by=<管理员>` 审计日志（logs/hbbs.log）。
- 控制台：用户管理区新增「新建用户」表单（用户名 + 密码 + 新建按钮），在用户列表上方；建号成功后刷新列表、清空表单并显示「用户已创建」；失败（重名、密码太短、用户名不合法）在表单下方内联显示错误，不弹窗。中英文字段齐全（随控制台语言切换）。
- 升级：停服 → 只换 hbbs → 启动（hbbr、rdsvc、脚本、地理库均不变）。
- 验证：新增 6 个单元测试（管理员建号成功且不签发会话、管理员会话仍有效；非管理员 403；未登录 401；重名、密码太短、用户名不合法均 400），cargo test 92/92（debug 与 release；lib 77 + hbbr 14 + 集成 1）；新增用户管理端到端 33/33（debug 与 release 各两遍：管理员建号、新用户网页/客户端登录、管理员会话存活、403/401 与校验错误、注册关闭场景下建号、旧注册路径仍顶掉会话的对照、管理面拆分下仅 console 地址可用、审计日志 `[auth] user-created`）；新增控制台界面端到端 19/19（debug 与 release 各两遍：真实 Chromium——表单渲染、填表建号、成功提示与列表刷新、表单清空、重名/短密码内联错误、中英文切换）；全量回归 60 次运行全绿（控制台 28/28 ×2、模板 17/17 ×2、锚定 17/17 ×2、配置 38/38 ×2、专项 69/69 ×2、就近选路 12/12 ×2、下载 50/50 ×2、语言切换 43/43 ×2、必须登录 21/21 ×2、中继 68/68 ×2、KX 25/25 ×2、hash 路由 38/38 ×2、最近中继 25/25 ×2、用户管理 33/33 ×2、用户管理界面 19/19 ×2），debug 与 release 均通过；ARM 两个变体 qemu 控制台套件 28/28 ×2。本版 Rust 改动集中在 src/api.rs 与控制台前端 src/api_pages.rs（全平台统一逻辑），无新增 Windows/release 专属分支；hbbr 不动。
- 真机确认项：控制台用户管理建一个新账号，新账号能登录，管理员登录态不被顶掉。

## v40：连接审计日志（Fixes #7）

- 背景（2026-11-17，GitHub issue #7）：被控端官方客户端在每次会话时都会向 `{API}/api/audit/conn` 投稿三类记录——连接建立（`new`）、登录认证（auth）、连接关闭（`close`），但本服务端没有实现该接口，一律 404（客户端日志里常见的 `Audit post dropped ... status 404` 就是这个），会话记录全部丢弃。本版接住这些记录，存库并在控制台展示，补齐审计能力。
- 新增：审计接收入口（API 管理面，客户端协议接口）。`POST /api/audit/conn`，请求体为 JSON，无鉴权 token，字段与官方客户端 `post_conn_audit` 一致：`id`（被控端设备 ID，字符串或数字）、`uuid`（base64）、`conn_id`（整数）、`session_id`（字符串）、`nonce`（每条记录唯一的 UUID v4）、`action`（`new` / `close`，auth 事件没有该字段）、`ip`（`new` 事件上的对端 IP）、`peer`（auth 事件上的 `[控制端 ID, 名称]`）、`type`（连接类型）、`primary_auth` / `two_factor`（可选整数）、`conn_audit_ref`（可选字符串）。服务端按 (id, uuid) 在 devices 表里校验——心跳已把设备 ID 与 uuid 绑定，未知设备或 uuid 不匹配返回 403 `{"error": ...}`（4xx 表示确定性拒绝，客户端不重试）；JSON 畸形或缺 id/nonce 返回 400。成功与重复 nonce 都返回 HTTP 200 且**空 body**（客户端契约：2xx + 空 body 才算成功）；服务端内部失败返回 200 + `{"error": ...}`（客户端会重试）。注意：设备在首次心跳落地之前发起的会话，其审计记录会被 403 丢弃——属已知边界，首次心跳之后即正常。
- 新增：审计存储。`audit_conn` 表（启动时自动建表）：nonce 唯一索引去重（永久去重，是官方 5 分钟 nonce 窗口的超集——客户端只在 120 秒内重试，所以永久去重不会误杀；insert-or-ignore 实现）、原始 JSON 全量存入 raw 字段备查、received_at 为服务端接收时间（Unix 秒）。本版不做自动清理（行很小，保留/清理策略留待后续版本）。
- 新增：审计查询接口（控制台管理面，需登录）。`GET /api/audit/sessions`，按 (device_id, conn_id) 把事件组装成会话：开始时间取 `new` 事件的 received_at（缺 `new` 时取最早事件）、结束时间取 `close` 事件的 received_at（无 close 即进行中，时长为空）、控制端 IP 取 `new` 事件的 ip、控制端 ID/名取 auth 事件的 peer、被控端主机名与控制端用户名通过 devices 表 LEFT JOIN（未绑定设备的用户名为空，页面显示 ID/名）。权限与 v9 一致：管理员看全部，普通用户只看控制端或被控端绑定到自己的会话。查询参数：`q`（控制端/被控端 ID 或名、IP 的子串匹配）、`days`（默认 30，0 = 全部）、`page`（默认 1）、`per_page`（默认 20，上限 100）；返回 `{sessions, total, page, per_page}`，按开始时间倒序。
- 新增：中继/直连推断。`new` 事件的 ip 是被控端 socket 的对端：中继会话时是对端中继的 IP，直连时是控制端的 IP。服务端取 `--relay-servers` 启动参数里的中继地址（域名用 ToSocketAddrs 解析，解析结果内存缓存 5 分钟），new.ip 命中中继 IP 即判为中继，否则判为直连；中继列表无法确定时 relayed 为空（页面显示「—」，不猜）。限制：中继地址解析依赖 DNS/启动参数，若中继换了 IP 或走非 RELAY_SERVERS 的中继，推断可能不准；直连会话中控制端 IP 恰好等于某中继 IP 时会误判为中继（极小概率）。
- 新增：控制台「审计日志」区（导航 + hash 路由 `#/audit`，中英文齐全）。表格列：开始时间 | 控制端（ID + 用户名/名）| 被控端（ID + 主机名）| 控制端 IP | 中继/直连 | 时长 | 状态（进行中/已结束）；筛选：搜索框（q）、时间范围（今天/7天/30天/全部）、分页（20 条/页）。进行中的会话时长实时递增。权限由服务端接口强制，页面只渲染返回的数据。
- 范围说明：本版只做 conn 审计。`/api/audit/file` 与 `/api/audit/alarm` 未实现（仍 404），作为后续工作。
- 另：设置页的注册开关提示已更新——关闭注册后管理员在「用户管理」可建号（v39 功能），不再是旧的「登录状态下打开注册页」说法。
- 升级：停服 → 只换 hbbs → 启动（hbbr、rdsvc、脚本、地理库均不变）。
- 验证：新增 7 个单元测试（正常投稿 200 空 body 且入库、重复 nonce 幂等、未知设备 403、畸形 400、会话组装含进行中会话的空时长、中继 IP 匹配与无中继列表时为空、查询权限范围），cargo test 99/99（debug 与 release；lib 84 + hbbr 14 + 集成 1）；新增审计端到端 45/45（debug 与 release 各两遍：真实 new/auth/close 序列——中继/直连/进行中/数字 ID 归一化、重复 nonce、403/400、会话组装与排序、中继标志与用户名映射、搜索/分页/权限范围、管理面拆分下投稿走 API 端口、查询走 console 端口、无中继列表时 relayed 为空）；新增控制台界面端到端 41/41（debug 与 release 各两遍：真实 Chromium——审计页渲染、23 条会话的表格与分页、搜索与时间筛选、中英文切换）；全量回归 68 次运行全绿（控制台 28/28 ×2、模板 17/17 ×2、锚定 17/17 ×2、配置 38/38 ×2、专项 69/69 ×2、就近选路 12/12 ×2、下载 50/50 ×2、语言切换 43/43 ×2、必须登录 21/21 ×2、中继 68/68 ×2、KX 25/25 ×2、hash 路由 38/38 ×2、最近中继 25/25 ×2、用户管理 33/33 ×2、用户管理界面 19/19 ×2、审计 45/45 ×2、审计界面 41/41 ×2），debug 与 release 均通过；ARM 两个变体 qemu 控制台套件 28/28 ×2。本版 Rust 改动集中在 src/api.rs 与控制台前端 src/api_pages.rs（全平台统一逻辑），无新增 Windows/release 专属分支；hbbr 不动。
- 真机确认项：真实会话后，控制台审计日志里出现该会话（开始时间、控制端/被控端、中继/直连、时长正确）；中继会话显示「中继」，直连显示「直连」；客户端日志里不再出现 `Audit post dropped ... status 404`。

## v40.1（预览版）：地址簿自动加入已绑定设备（Fixes #8）

- 背景（2026-11-17，GitHub issue #8）：两点建议。(1) 地址簿字段与 lejianwen/sctgdesk 对齐（forceAlwaysRelay、rdpPort/rdpUsername/loginName、same_server、password 等字段）。(2) 已绑定到账号的设备自动进入该账号的地址簿（建议用「登录设备」分组）。
- 字段对齐说明：服务端 `ab_store` 只存客户端 `data` 字符串原样（不透明 blob，不解析 peer 字段），所以客户端已有的全部 peer 字段本来就原样往返，存储层无需改动。本版用端到端测试验证：15 个字段的 peer 投稿再取回，字节完全一致。
- 新增：`GET /api/ab` 合并已绑定设备。服务端取回调用者已存的地址簿后，把绑定到该账号的设备（devices 表，username == 调用者）追加进 peers：id 取设备 ID，username/hostname/platform 取心跳上报的系统信息（platform 由 os 映射：Windows/Linux/Mac OS/Android/iOS，未知 os 原样保留），tags 为 `["登录设备"]`，forceAlwaysRelay 为 `"false"`，hash/password/rdpPort/rdpUsername/loginName 为空，same_server 为 null；tags 列表补上「登录设备」。用户已有同 ID 的条目时保留用户自己的条目原样（不加标签，不改字段，不产生重复）。
- 新增：无已存地址簿时合成返回。调用者从未投稿过地址簿但有已绑定设备时，返回合成的 `{"tags": ["登录设备"], "peers": [...]}`；既无地址簿又无已绑定设备时仍返回字面 `null`（旧契约不变）。
- 新增：`POST /api/ab` 上传时剥离服务端管理的条目。投稿的 peers 中带「登录设备」标签的条目在入库前剔除（下次 GET 时按实时绑定状态重新加入，解绑后自动消失）；其余条目原样入库。投稿数据不是合法 JSON 时原样入库（容错，不破坏旧客户端）。「登录设备」标签由服务端管理：用户手动给某条目打上该标签，下次同步时会被剥离。
- 容错：已存地址簿解析失败时原样返回，不合并（不破坏现有客户端）；合并后的 JSON 经 serde_json 重新序列化，对象键按字母序排列（语义不变，客户端不依赖键顺序）；无合并/无剥离时返回原字符串原样。
- 限制：自动加入的条目不带密码与 hash，用户在客户端里自行设置；platform 由 sysinfo 的 os 映射，os 字符串无法识别时原样保留；本版无控制台改动，无开关（功能直接在客户端地址簿里呈现）。
- 升级：停服 → 只换 hbbs → 启动（hbbr、rdsvc、脚本、地理库均不变）。本版是预览版：PRO_VERSION 仍为 40，一键更新不会推送预览版，须手动下载安装。
- 验证：新增 7 个单元测试（platform 映射、合并加入设备与标签、同 ID 去重、畸形 blob 透传、上传剥离、无 blob 合成与 null、接口级合并/剥离/权限范围），cargo test 106/106（debug 与 release；lib 91 + hbbr 14 + 集成 1）；新增地址簿端到端 27/27（debug 与 release 各两遍：15 字段往返字节一致、合并字段与标签、同 ID 条目保留、无 blob 合成与 null、畸形 blob 原样返回、上传剥离与存储检查、解绑自愈、权限范围、401）；全量回归 72 次运行全绿（控制台 28/28 ×2、模板 17/17 ×2、锚定 17/17 ×2、配置 38/38 ×2、专项 69/69 ×2、就近选路 12/12 ×2、下载 50/50 ×2、语言切换 43/43 ×2、必须登录 21/21 ×2、中继 68/68 ×2、KX 25/25 ×2、hash 路由 38/38 ×2、最近中继 25/25 ×2、用户管理 33/33 ×2、用户管理界面 19/19 ×2、审计 45/45 ×2、审计界面 41/41 ×2、地址簿 27/27 ×2），debug 与 release 均通过；ARM 两个变体 qemu 控制台套件 28/28 ×2。本版 Rust 改动集中在 src/api.rs（全平台统一逻辑），无新增 Windows/release 专属分支；hbbr 不动。
- 真机确认项：客户端登录后，地址簿里出现已绑定设备（「登录设备」分组），主机名/平台正确；客户端里给自动加入的条目设密码后能正常连接；解绑设备后地址簿里该条目消失。

## v41：自助修改密码（Fixes #9）

- 背景（2026-11-17，GitHub issue #9）：普通用户看不到用户管理模块，没有修改密码的入口；全项目唯一的改密码接口是管理员专用的 `POST /api/admin/users/:name/password`。改密码只能找管理员重置。
- 新增：自助改密码接口 `POST /api/auth/change-password`（控制台管理面，任意登录用户可用）。请求体 `{"old_password": "...", "new_password": "..."}`。服务端先校验旧密码（bcrypt），旧密码不对返回 400「旧密码不正确」；新密码至少 6 位，不足返回 400。成功返回 `{"ok": true}`，并记一条 `[auth] password-changed ... other_sessions_revoked=<N>` 审计日志（logs/hbbs.log）。未登录返回 401。旧密码错误返回 400 而不是 401：控制台的 api() 辅助函数把任何 401 当作「会话过期」跳回登录页，用 400 才能在表单内显示错误。
- 安全：接口只改调用者自己的密码（取自登录会话，不接受用户名字段），且必须校验旧密码。管理员重置接口不对普通用户放开：放开会让任意登录用户改别人的密码。
- 防暴力破解：旧密码验证失败计入 AuthGuard（与登录同一套按账号/按 IP 计数与锁定规则）；连续失败 5 次后账号锁定 15 分钟（起步，重复锁定翻倍，上限 12 小时），锁定期内返回 429；改密码成功后清空账号失败计数（与登录成功一致）。
- 会话处理：改密码成功后吊销调用者的其他网页会话（其他设备/浏览器需用新密码重新登录），当前会话保持有效。用 API Token 调用的改密码没有当前网页会话，该账号的全部网页会话都会吊销。
- 新增：控制台入口。顶栏（用户名旁边）加「修改密码」按钮，所有登录用户可见（不带 adminonly）；点开表单填旧密码、新密码、确认新密码，成功/失败都在表单内提示。中英文切换覆盖全部新文案（12 个新键，字典 331/331 对齐）。
- 限制：客户端 access_token 不受影响（改密码后已登录的客户端保持登录状态，登出重登时才需要新密码）；被禁用的账号无法登录，自然无法调用本接口。
- 升级：停服 → 只换 hbbs → 启动（hbbr、rdsvc、脚本、地理库均不变）。一键更新推送本版；手动安装到 Release 页面下载对应平台的包，解压后替换 hbbs。
- 验证：新增 6 个单元测试（改密码成功更新 hash 并吊销其他会话、旧密码错误拒绝、新密码过短拒绝、未登录 401、连续失败锁定、成功后清空失败计数），cargo test 112/112（debug 与 release；lib 97 + hbbr 14 + 集成 1）；新增自助改密码端到端 40/40（debug 与 release 各两遍：基本流程、会话吊销、AuthGuard 锁定、管理面拆分）；新增控制台界面端到端 30/30（debug 与 release 各两遍：普通用户可见入口、管理区隐藏、表单校验、旧密码错误不跳转登录、改密码成功、中英文切换）；全量回归 80 次运行全绿（控制台 28/28 ×2、模板 17/17 ×2、锚定 17/17 ×2、配置 38/38 ×2、专项 69/69 ×2、就近选路 12/12 ×2、下载 50/50 ×2、语言切换 43/43 ×2、必须登录 21/21 ×2、中继 68/68 ×2、KX 25/25 ×2、hash 路由 38/38 ×2、最近中继 25/25 ×2、用户管理 33/33 ×2、用户管理界面 19/19 ×2、审计 45/45 ×2、审计界面 41/41 ×2、地址簿 27/27 ×2、自助改密码 40/40 ×2、自助改密码界面 30/30 ×2），debug 与 release 均通过；ARM 两个变体 qemu 控制台套件 28/28 ×2。本版 Rust 改动集中在 src/api.rs 与 src/api_pages.rs（全平台统一逻辑），无新增 Windows/release 专属分支；hbbr 不动。
- 真机确认项：普通用户在控制台点「修改密码」，旧密码正确时改密码成功，新密码能登录，旧密码不能登录；其他浏览器/设备的登录失效；旧密码错误时页面内提示错误，不跳回登录页。

## v42：控制台体验修复 + 模板版本检测

- 背景（2026-11-17，用户在真实控制台反馈）：两处体验问题。(1) 点顶栏「修改密码」打开面板后没有退出入口，误点只能刷新页面；用户管理页一点进就直接显示新建用户表单。(2) 模板下载不管本地是不是最新版，点下载/更新都会强制重新下载。
- 新增：改密码面板可退出。面板里「确认修改」旁边加「取消」按钮，点一下收起面板并清空三个输入框与提示；顶栏按钮仍可开关面板。
- 新增：用户管理列表为主视图。页面先显示用户列表，新建用户表单收到「新建用户」按钮后面；点开表单填用户名、密码，「创建」提交，「取消」收起并清空。创建成功后表单保持打开（可继续新建），列表自动刷新。
- 新增：模板下载跳过已是最新版的情况。点「下载/更新模板」后，服务端先解析要下载的版本（版本留空 = 官方最新版，或填指定版本），再与本地存的模板版本（tpl_version）比对：文件存在且版本一致时不下载，控制台显示「已是最新版本（1.5.3），无需下载」。本地没存过版本号（手动放的模板文件）或模板文件缺失时照常下载。
- 文案：中英文切换覆盖全部新文案（3 个新键：app.cancel、usr.create_submit、set.tpl_uptodate，字典 334/334 对齐）。
- 升级：停服 → 只换 hbbs → 启动（hbbr、rdsvc、脚本、地理库均不变）。一键更新推送本版；手动安装到 Release 页面下载对应平台的包，解压后替换 hbbs。
- 验证：cargo test 112/112（debug 与 release；lib 97 + hbbr 14 + 集成 1）；模板端到端 40/40（debug 与 release 各两遍：版本一致跳过、版本不同下载、无版本号下载、文件缺失下载）；控制台界面端到端：用户管理界面 34/34、改密码界面 37/37、模板版本检测界面 18/18（debug 与 release 各两遍）；全量回归 80 次运行全绿（控制台 28/28 ×2、模板 40/40 ×2、锚定 17/17 ×2、配置 38/38 ×2、专项 69/69 ×2、就近选路 12/12 ×2、下载 50/50 ×2、语言切换 43/43 ×2、必须登录 21/21 ×2、中继 68/68 ×2、KX 25/25 ×2、hash 路由 38/38 ×2、最近中继 25/25 ×2、用户管理 33/33 ×2、用户管理界面 34/34 ×2、审计 45/45 ×2、审计界面 41/41 ×2、地址簿 27/27 ×2、自助改密码 40/40 ×2、自助改密码界面 37/37 ×2），debug 与 release 均通过；ARM 两个变体 qemu 控制台套件 28/28 ×2。本版 Rust 改动集中在 src/api.rs 与 src/api_pages.rs（全平台统一逻辑），无新增 Windows/release 专属分支；hbbr 不动。
- 真机确认项：控制台里点「修改密码」，「取消」收起面板；用户管理页先看到用户列表，点「新建用户」才出现表单；模板已是最新版时点「下载/更新模板」，页面提示已是最新版本且不重新下载。

## v43：审计会话回收 + 中继推断 + 控制台表格重建

- 背景（2026-11-19，issue #10 与 #11）：(1) 审计日志里丢了关闭记录的会话一直停在「进行中」，越积越多；中继/直连列一直是「—」，因为判断中继的 IP 集合只从 --relay-servers 取，没算上本机的公网地址。(2) 控制台表格在窄屏下按钮竖排、列挤在一起，看不清。
- 新增：审计会话回收。被控端每次心跳上报当前连接的 conn_id 列表；服务端发现某条审计会话的 conn_id 连续 60 秒不在心跳里，就把它记为已结束（closed_by=stale，写 audit_session_close 表），控制台显示「已结束（无关闭记录）」；conn_id 重新出现在心跳里时撤销回收，会话恢复进行中。close 事件优先于回收记录；回收记录只在晚于该会话最后一条事件时生效（客户端重启后 conn_id 可能复用）。
- 新增：离线规则。设备 last_seen 超过 24 小时未更新时，它的未结束会话在读取时按已结束算（closed_by=offline，结束时间取 last_seen 与开始时间的较大值），控制台显示「已结束（设备离线）」；不写库，设备重新上线后心跳规则接手。
- 新增：中继推断。判断中继的 IP 集合在 --relay-servers 之外加入 public_host（控制台「公网地址」设置，与 client_config() 同源）；控制端 IP 落在集合里记中继，否则记直连；两者都未配置时仍显示「—」。改 public_host 后下一次查询即生效（缓存键包含 relay-servers 与 public_host）。
- 新增：控制台表格按 modern-web-guidance responsive-table 指南重建。每张表包在 .table-wrapper 滚动容器里，表头粘性固定；窄于 600px 时表变堆叠卡片，列名由 --label-N 变量注入；长单元格截断加省略号，完整文本保留在 DOM 里；按钮不再竖排。
- 文案：中英文切换覆盖全部新文案（2 个新键：aud.closed_stale、aud.closed_offline，字典 336/336 对齐）。
- 升级：停服 → 只换 hbbs → 启动（hbbr、rdsvc、脚本、地理库均不变）。一键更新推送本版；手动安装到 Release 页面下载对应平台的包，解压后替换 hbbs。
- 验证：cargo test 121/121（debug 与 release 各两遍；lib 106 + hbbr 14 + 集成 1）；模板端到端 40/40（debug 与 release 各两遍）；控制台界面端到端：用户管理界面 34/34、改密码界面 37/37、模板版本检测界面 18/18（debug 与 release 各两遍）；审计回收端到端 26/26（debug 与 release 各两遍）；控制台布局 294/294（1280px 96 + 800px 89 + 480px 102，debug 与 release 各两遍）；全量回归 88 次运行全绿（控制台 28/28 ×2、模板 40/40 ×2、锚定 17/17 ×2、配置 38/38 ×2、专项 69/69 ×2、就近选路 12/12 ×2、下载 50/50 ×2、语言切换 43/43 ×2、必须登录 21/21 ×2、中继 68/68 ×2、KX 25/25 ×2、hash 路由 38/38 ×2、最近中继 25/25 ×2、用户管理 33/33 ×2、用户管理界面 34/34 ×2、审计 45/45 ×2、审计界面 41/41 ×2、地址簿 27/27 ×2、自助改密码 40/40 ×2、自助改密码界面 37/37 ×2、审计回收 26/26 ×2、控制台布局 294/294 ×2），debug 与 release 均通过；ARM 两个变体 qemu 控制台套件 28/28 ×2。本版 Rust 改动集中在 src/api.rs 与 src/api_pages.rs（全平台统一逻辑），无新增 Windows/release 专属分支；hbbr 不动。
- 真机确认项：审计日志里一条丢了关闭记录的会话在设备下次心跳后变成「已结束（无关闭记录）」；离线超过 24 小时的设备会话显示「已结束（设备离线）」；走 VPS 中继的会话在「中继」列显示中继而不是「—」；手机窄屏上控制台表格呈卡片式，按钮横排。

## v44：检测更新按钮修复 + API Token 创建后改名 + 表格截断格点展

- 背景（2026-11-19，真机反馈）：(1) 设置页点检测更新，显示「已是最新版本」后按钮一直灰到刷新。(2) API Token 创建后横幅没有关闭入口，建好的 token 改不了名，名称框里填的名字只进下一次创建。(3) 设备列表系统列截断后全文只能悬停看，触屏/键盘够不着。
- 修复：检测更新按钮。`$('upd-check').onclick` 的 `!j.ok` 与 `!j.has_update` 提前 return 跳过了 `btn.disabled=false`；改为 `finally` 里恢复按钮，所有退出路径（无更新、失败、网络异常）都恢复可用。
- 新增：Token 改名。`tokens_create` 返回新 token 的 id；新增 `PATCH /api/tokens/:id`，仅限属主改名（`where id=? and username=?`，别人的 token 返回 404）；空名称按创建逻辑记为 "default"。`#tok-new` 横幅加名称输入框（预填创建时用的名）+ 保存名称按钮 + 关闭按钮；创建成功后名称输入框清空。
- 新增：表格截断格点展。点被截断的格子展开全文（本列内折行，列宽不动），再点收起；真正被截断的格子才可交互（`scrollWidth > clientWidth` 判定，加 `tabindex="0"` + `aria-expanded`），鼠标、键盘（Enter/Space）、触屏都可用；`title` 属性保留。按 modern-web-guidance single-line-overflow 指南做（"DO NOT rely on `title` alone: it is unreachable from the keyboard and on touch."）。
- 文案：中英文切换覆盖全部新文案（1 个新键：tok.save_name，字典 337/337 对齐）。
- 升级：停服 → 只换 hbbs → 启动（hbbr、rdsvc、脚本、地理库均不变）。一键更新推送本版；手动安装到 Release 页面下载对应平台的包，解压后替换 hbbs。
- 验证：cargo test 123/123（debug 与 release 各两遍；lib 108 + hbbr 14 + 集成 1）；更新按钮端到端 12/12、Token 改名 HTTP 19/19、Token 改名界面 17/17、截断展开 30/30（debug 与 release 各两遍）；全量回归 104 次运行全绿（控制台 28/28 ×2、模板 40/40 ×2、锚定 17/17 ×2、配置 38/38 ×2、专项 69/69 ×2、就近选路 12/12 ×2、下载 50/50 ×2、语言切换 43/43 ×2、必须登录 21/21 ×2、中继 68/68 ×2、KX 25/25 ×2、hash 路由 38/38 ×2、最近中继 25/25 ×2、用户管理 33/33 ×2、用户管理界面 34/34 ×2、审计 45/45 ×2、审计界面 41/41 ×2、地址簿 27/27 ×2、自助改密码 40/40 ×2、自助改密码界面 37/37 ×2、审计回收 26/26 ×2、控制台布局 294/294 ×2、更新按钮 12/12 ×2、Token 改名 19/19 ×2、Token 改名界面 17/17 ×2、截断展开 30/30 ×2），debug 与 release 均通过；ARM 两个变体 qemu 控制台套件 28/28 ×2。本版 Rust 改动集中在 src/api.rs 与 src/api_pages.rs（全平台统一逻辑），无新增 Windows/release 专属分支；hbbr 不动。
- 真机确认项：无更新结果后检测更新按钮能再点；创建 token 后在横幅里改名，列表里名称更新，关闭按钮清掉横幅；设备列表里点系统列的截断格展开全文，再点收起。

## v45：中继详情地址回退 + API key 页重做 + API 监听默认 IPv6 + Docker 镜像

- 背景（2026-11-21）：(1) 中继节点详情页的地址一直不上报——数据其实每次心跳都在收（来源 IP 与监听端口），缺的只是详情页的回退显示与端口行。(2) v44 的 Token 横幅预填了 "default"，打字会拼成 "default11111"；页面术语与操作需要收拢。(3) issue #12 第一部分：Docker on ARM iStoreOS 下 IPv6 不通，--api-listen 默认只绑 IPv4。(4) Docker 部署需要官方镜像与本地构建脚本。
- 新增：中继详情页地址回退。节点详情的地址在未配置时回退到最近一次心跳的来源 IP（last_ip），并新增监听端口行。
- 新增：API Token 页重做。界面统称 API key；创建与改名共用一个名称框；创建按钮改叫「添加 API key」；创建时不再预填 "default"（修掉预填值与输入拼接的坑）；关闭按钮清掉横幅。
- 新增：--api-listen 默认 [::]:21114（Refs #12）。API/控制台监听默认绑 IPv6 通配地址，IPv6 客户端可直连；绑不上 IPv6 时（主机无 IPv6 或地址未分配）自动回退到同端口 0.0.0.0 并打一条警告，不影响启动。回退逻辑与 --console-listen 平面共用，其默认值不变；21116/21118 会合监听不动。start.sh/start.ps1 与 rdsvc 改传 [::]:$API_PORT。issue #12 的双 IPv6 部分（br-lan 与 pppoe-wan 两个 IPv6 地址）仍在等报告者日志，issue 保持开启。
- 集成修复：(1) TLS 分支的监听 socket 在交给 tokio 前需设为 nonblocking，否则首次 accept 时 panic（v23 HTTPS 端到端抓到）；(2) last_ip 记录时把 ::ffff: 映射地址还原为纯 IPv4，否则中继详情页显示 ::ffff: 地址（中继详情端到端抓到）。
- 新增：Docker 镜像。GitHub Actions 在版本标签推送后自动构建并发布到 GHCR：ghcr.io/wirh/rustdesk-server-pro（s6 镜像）与 ghcr.io/wirh/rustdesk-server-pro-classic（classic 镜像）；整数版标签（如 v45）移动 latest 与主版本标签，预览版标签（如 v45.1）不移动 latest。本地构建：./docker/build-local.sh，默认用 ./dist-pro 里最新的 linux-x86_64-glibc 包，加 --classic 构建 classic 镜像。Docker Hub 同步发布：tesx/rustdesk-server-pro 与 tesx/rustdesk-server-pro-classic。
- 文案：中英文切换覆盖全部新文案（1 个新键：app.close，字典 338/338 对齐）。
- 升级：停服 → 换 hbbs + Windows 服务换 rdsvc.exe + 启动脚本 → 启动（hbbr、地理库不变）。旧 rdsvc 显式传 0.0.0.0 会覆盖新默认，不换则 [::] 修复不生效；Linux start.sh 同理。一键更新只换 hbbs，rdsvc 与脚本需手动换；手动安装到 Release 页面下载对应平台的包，解压后替换 hbbs、rdsvc.exe 与启动脚本。
- 验证：cargo test 128/128（debug 与 release 各两遍；lib 113 + hbbr 14 + 集成 1）；API key HTTP 25/25、API key 界面 35/35、中继详情界面 30/30、监听 9/9（debug 与 release 各两遍）；全量回归 116 次运行全绿（控制台 28/28 ×2、模板 40/40 ×2、锚定 17/17 ×2、配置 38/38 ×2、专项 69/69 ×2、就近选路 12/12 ×2、下载 50/50 ×2、语言切换 43/43 ×2、必须登录 21/21 ×2、中继 68/68 ×2、KX 25/25 ×2、hash 路由 38/38 ×2、最近中继 25/25 ×2、用户管理 33/33 ×2、用户管理界面 34/34 ×2、审计 45/45 ×2、审计界面 41/41 ×2、地址簿 27/27 ×2、自助改密码 40/40 ×2、自助改密码界面 37/37 ×2、审计回收 26/26 ×2、控制台布局 294/294 ×2、更新按钮 12/12 ×2、Token 改名 19/19 ×2、截断展开 30/30 ×2、API key 25/25 ×2、API key 界面 35/35 ×2、中继详情界面 30/30 ×2、监听 9/9 ×2），debug 与 release 均通过；ARM 两个变体 qemu 控制台套件 28/28 ×2。本版 Rust 改动集中在 src/api.rs、src/api_pages.rs 与 src/main.rs（全平台统一逻辑），无新增 Windows/release 专属分支；hbbr 不动；Windows 与 ARM 交叉构建在 vendored OpenSSL 下全部通过（musl 不再需要目标专属 OPENSSL_DIR）。
- 真机确认项：中继详情页显示心跳来源 IP 与监听端口；API key 页创建/改名/关闭操作正常；用户服务器上 [::] 监听生效（IPv6 客户端能连控制台，IPv4 客户端不受影响）；Docker 镜像在 v45 标签推送后由 CI 构建发布到 GHCR 与 Docker Hub。

## v46：控制台页脚关于项目 + 侧边栏导航 + API key 备注 + 中继自报公网 IP + 检测预览版 + 审计日期范围 + 按地区下载顺序

- 背景（2026-11-21）：(1) 控制台缺项目说明，导航横排占顶栏。(2) API key 页的名称语义不清，备注应跟在 key 后面改。(3) 中继详情的地址只能看到心跳来源 IP，家用路由器改写后是内网 IP，agent 需自报公网 IP。(4) 预览版只能手动下载，一键更新不提供。(5) 审计日志只能按相对天数查，缺显式起止日期。(6) 海外服务器模板下载先试 5 个国内镜像才直连，慢。
- 新增：控制台页脚「关于项目」。页面底部新增关于项目块：简介与项目主页链接（github.com/wirh/rustdesk-server-pro），中英文案按定稿。
- 新增：侧边栏导航。顶部横排导航改为左侧竖直可折叠侧边栏：分组导航项（设备与审计 / 管理与工具）+ 折叠控制，折叠状态存 localStorage，刷新后保持；窄屏（1024px 以下）自动收为图标栏，导航仍可一键到达；顶栏保留当前页标题与语言、改密码、退出。按 modern-web-guidance navigation-drawer（sidebar 变体）与 responsive-design（100dvh）指南做。
- 新增：API key 页名称改为备注。备注输入框移到横幅里、紧跟在 API key 后面；保存备注与关闭在同一行；创建时不发名称（服务端默认 "default"），备注在创建后通过横幅里的保存备注设置；静态表单只留「添加 API key」按钮；列表首列标题随之变备注。API/DB 的 name 字段不变。
- 新增：中继 agent 自报公网 IP。agent 启动时通过外部 IP 查询服务检测自身公网 IP（api.ipify.org → ifconfig.me/ip → icanhazip.com，多服务 fallback，单个 5 秒超时；每小时重检，失败保留上次已知值），随心跳上报；服务端新增 relay_nodes.public_ip 列（启动时自动迁移）并存储展示；可用 RELAY_PUBLIC_IP_URLS 环境变量覆盖查询服务（空值关闭检测）。控制台中继地址显示优先级：注册地址 > agent 上报的公网 IP > 心跳来源 IP；旧版 agent 不上报时回退到心跳来源 IP。
- 新增：更新区「检测预览版」复选框。勾选后更新检查包含预览版（vN.M 标签），提供最新版本，下载与重启更新流程对预览版可用；复选框状态存 localStorage，每次检查请求带 include_prerelease，服务端无状态；默认不勾选。预览版构建通过构建时环境变量 PRO_FULL_VERSION 把完整版本号 baked 进二进制（正式版不设该变量，行为不变），装上预览版后更新器不再重复提示同一预览版；版本比较与下载校验支持 vN.M 标签。预览版打包时需在构建 hbbs 前 export PRO_FULL_VERSION=<完整版本号，如 46.1>；CI 的 Docker 镜像构建尚未接入该变量（后续跟进），预览版 Docker 镜像暂时不带完整版本号。
- 新增：审计日志日期范围选择器。审计页增加起止日期查询（原生 <input type="date">）；GET /api/audit/sessions 新增 from/to 参数（YYYY-MM-DD），起止日整日包含（00:00:00–23:59:59），单边为另一端开放，非法日期或起日晚于止日返回 400；日期按服务端本地时间解释；指定日期范围时忽略相对天数。
- 新增：按地区下载顺序。模板下载候选源按服务器所在地区排序：国内加速镜像优先，国外直连 GitHub 优先；慢源仍按 stall 检测（20 秒无数据或 15 秒内低于约 10KB/s）自动 failover 到下一个；自定义镜像（管理员设置）始终排第一。地区检测信号优先级：TZ 环境变量 → /etc/localtime → /etc/timezone → 系统 UTC 偏移；无法判断时保持原顺序（镜像优先）。一键更新器的下载共享同一候选顺序，版本信息请求同样走该顺序。
- 文案：中英文切换覆盖全部新文案（11 个新键：about.* 3、nav.g_devices/nav.g_manage/nav.collapse/nav.expand/nav.label 5、set.upd_prerelease 1、aud.from/aud.to 2；tok.* 3 个键改名，字典 349/349 对齐）。
- 升级：停服 → 只换 hbbs → 启动（hbbr、rdsvc、启动脚本、地理库不变）；一键更新或手动下载都可。**中继节点需手动换 relay-agent 二进制**（v43–v45 不需要，这次需要）：停 agent 服务 → 替换 relay-agent → 启动；hbbr、agent.json、relay.env、密钥文件不动；重启后首次心跳即上报公网 IP。未更新的中继节点显示与之前一致（心跳来源 IP），功能不受影响。
- 验证：cargo test 138/138（debug 与 release 各两遍；lib 123 + hbbr 14 + 集成 1）；Go relay-agent 测试 35/35（go vet 与 gofmt 干净）；页脚 27/27、侧边栏 77/77、中继公网 IP 21/21、备注 HTTP 27/27、备注界面 37/37、预览更新 35/35（主流程 29 + 预览版构建 6）、审计日期 23/23、审计日期界面 22/22、地区下载 32/32（debug 与 release 各两遍）；全量回归 148 次运行全绿（控制台 28/28 ×2、模板 40/40 ×2、锚定 17/17 ×2、配置 38/38 ×2、专项 69/69 ×2、就近选路 12/12 ×2、下载 50/50 ×2、语言切换 43/43 ×2、必须登录 21/21 ×2、中继 68/68 ×2、KX 25/25 ×2、hash 路由 38/38 ×2、最近中继 25/25 ×2、用户管理 33/33 ×2、用户管理界面 34/34 ×2、审计 45/45 ×2、审计界面 41/41 ×2、地址簿 27/27 ×2、自助改密码 40/40 ×2、自助改密码界面 37/37 ×2、审计回收 26/26 ×2、控制台布局 294/294 ×2、更新按钮 12/12 ×2、Token 改名 19/19 ×2、截断展开 30/30 ×2、API key 25/25 ×2、中继详情界面 30/30 ×2、监听 9/9 ×2、页脚 27/27 ×2、侧边栏 77/77 ×2、中继公网 IP 21/21 ×2、备注 HTTP 27/27 ×2、备注界面 37/37 ×2、预览更新 35/35 ×2、审计日期 23/23 ×2、审计日期界面 22/22 ×2、地区下载 32/32 ×2），debug 与 release 均通过（首轮 3 个套件的 9 次运行因测试侧问题失败：布局套件把图标按钮误判为换行、预览套件的版本号锚定停留在 v45、一次模板下载网络偶发；修测试后重跑通过）；ARM 两个变体 qemu 控制台套件 28/28 ×2。本版 Rust 改动集中在 src/api.rs、src/api_pages.rs 与 build.rs（全平台统一逻辑），Go 改动在 relayagent/（新增 publicip.go）；hbbr 不动；Windows 与 ARM 交叉构建全部通过。
- 真机确认项：页脚关于项目块显示正常；侧边栏展开/折叠与窄屏图标栏；API key 创建后在横幅里写备注，列表里备注更新；中继节点更新 agent 后详情页显示 VPS 公网 IP；勾选检测预览版后能收到预览版更新提示；审计页按日期范围查到对应会话；国内服务器模板下载仍走加速镜像优先。

## v46.1（预览版）：Docker 运行时数据落进数据卷（Fixes #13）

- 背景（2026-11-22）：issue #13（yguangg 报告）：s6 镜像运行时数据写进了容器内的 `/usr/bin`，`/data` 卷始终为空，重建容器丢数据；经典镜像同样受影响。根因：hbbs/hbbr 启动时把工作目录锚到二进制所在目录，CI 把二进制放在 `/usr/bin`，锚定抵消了 s6 运行脚本的 `cd /data`；key-secret 服务把密钥写到 `/data/id_ed25519`，hbbs 从工作目录读相对路径 `id_ed25519`，注入的密钥从未被读到。
- 修复：RD_SKIP_ANCHOR 环境变量。设为 `1` 时 hbbs/hbbr 跳过锚定，保持继承来的工作目录；未设或取其他值时行为不变，主机安装无需改动。s6 与经典镜像在镜像内设置 `RD_SKIP_ANCHOR=1`：s6 镜像工作目录 `/data`，经典镜像工作目录 `/root`，运行时数据（密钥、账号库、日志）落进数据卷；key-secret 注入的密钥现在能被读到。
- CI：预览版 Docker 镜像带完整版本号。build job 在预览标签（vN.M）推送时导出 PRO_FULL_VERSION（Cross.toml passthrough 进 cross 容器），build.rs 把完整版本号 baked 进 hbbs；正式标签与 dispatch 运行跳过该步骤。v46 备注的「预览版 Docker 镜像暂时不带完整版本号」已解决。smoke 扩展为双镜像数据落卷检查：s6 镜像 API 在 21114 应答，db_v2.sqlite3、id_ed25519(.pub)、api_v1.sqlite3、logs/hbbs.log 出现在 `/data` 卷，`/usr/bin` 里无运行时数据，注入的密钥对与启动日志的 Key 一致（日志含 "Private key comes from id_ed25519"，无 "Private/public key written"）；经典镜像同样五项数据出现在 `/root` 卷，API 应答。
- 升级：主机安装换 hbbs 即可，再启动（hbbr 同包新编，不设环境变量时行为与 v46 一致；rdsvc、启动脚本、地理库不变）；控制台勾选「检测预览版」后一键更新提供 v46.1，手动下载都可。Docker 用户升级前先按上文「升级到 v46.1（数据迁移）」把旧容器里 `/usr/bin` 下的数据拷到 `/data` 卷，再重建容器；否则重建后旧数据全部丢失。已部署的中继节点无需更新（relay-agent 未改动）。
- 验证：cargo test 140/140（debug 与 release 各两遍；lib 124 + hbbr 15 + 集成 1）；锚定跳过 57/57（debug 与 release 各两遍）；全量回归 152 次运行全绿（控制台 28/28 ×2、模板 40/40 ×2、锚定 17/17 ×2、配置 38/38 ×2、专项 69/69 ×2、就近选路 12/12 ×2、下载 50/50 ×2、语言切换 43/43 ×2、必须登录 21/21 ×2、中继 68/68 ×2、KX 25/25 ×2、hash 路由 38/38 ×2、最近中继 25/25 ×2、用户管理 33/33 ×2、用户管理界面 34/34 ×2、审计 45/45 ×2、审计界面 41/41 ×2、地址簿 27/27 ×2、自助改密码 40/40 ×2、自助改密码界面 37/37 ×2、审计回收 26/26 ×2、控制台布局 294/294 ×2、更新按钮 12/12 ×2、Token 改名 19/19 ×2、截断展开 30/30 ×2、API key 25/25 ×2、中继详情界面 30/30 ×2、监听 9/9 ×2、页脚 27/27 ×2、侧边栏 77/77 ×2、中继公网 IP 21/21 ×2、备注 HTTP 27/27 ×2、备注界面 37/37 ×2、预览更新 35/35 ×2、审计日期 23/23 ×2、审计日期界面 22/22 ×2、地区下载 32/32 ×2、锚定跳过 57/57 ×2），debug 与 release 均通过（中途服务重启打断运行，60 次后从断点续跑，全程零失败）；ARM 两个变体 qemu 控制台套件 28/28 ×2。CI dispatch 运行两次：首次（main @ c7341f5）四路构建、Docker 推送与 debian 打包全绿，PRO_FULL_VERSION 步骤按设计跳过，smoke 的 s6 镜像数据落卷检查通过，但密钥对齐断言因测试侧权限问题未能执行（runner 读不到容器内 root 属主的 id_ed25519.pub），classic 检查随前序失败被跳过；改成容器内读取后重跑（main @ aed5d5a）全绿，s6 与 classic 双镜像 smoke 均通过（含密钥对齐）。本版 Rust 改动集中在 src/common.rs（锚定跳过，全平台统一逻辑）与 CI/Docker 配置；relay-agent 未改动；Windows 与 ARM 交叉构建全部通过。
- 真机确认项：Docker 用户升级后数据在 `/data` 卷里、重建不丢；检测预览版勾选后收到 v46.1 提示且装上后不再重复提示。

## API 调用说明

服务端在 API 端口上提供 HTTP 接口：官方客户端用的登录/设备协议接口，以及控制台用的管理接口（生成客户端、设备列表、在线更新等）。本节以 src/api.rs 的实际路由表为准；示例中的 token、密钥已截断或替换为占位符，格式与真实返回一致。以下示例均已在本地 hbbs 上实测跑通（curl 实测：`/api/me`、`/api/devices`、`/api/client-config`、`/api/client-downloads`、`/api/admin/update/status`、`/api/tokens`、`/api/login`、`/api/peers`、`/api/users`、`/api/currentUser`、`/api/admin/gen-client`、`/api/admin/gen-clients`、下载链接及删除后失效）。

### Base URL

默认 `http://<服务器地址>:21114`（`--api-listen` 默认 `[::]:21114`，可改地址/端口，`off` 关闭）。开启 TLS（`--tls-cert` / `--tls-key`）后用 `https://`。管理面拆分后（`--console-listen`，见 v23 一节）注意区分两个地址：

- API 地址（21114）：客户端协议接口（`/api/login`、`/api/peers`、`/api/users`、`/api/currentUser`、`/api/heartbeat`、`/api/sysinfo`、`/api/ab`、`/api/sysinfo_ver`、`/api/oidc/*`）+ 下载链接 `/dl/*` + `/oidc/login`；
- Console 地址：控制台页面（`/console/*`）与管理接口（`/api/auth/*`、`/api/me`、`/api/client-config`、`/api/admin/*`、`/api/devices`、`/api/tokens*`）。

拆分后调管理接口必须用 console 地址和端口，用 API 地址会 404。未拆分时两者在同一个地址上。

除 `/dl/*`（文件流）与 `/api/sysinfo_ver`（纯文本 `1`）外，所有接口的请求与响应都是 JSON；POST 请求请带 `Content-Type: application/json`。

### 鉴权：三种凭证，互不通用

1. **API Token（`rdt_`，脚本/程序调用推荐）**：在控制台「API Token」页创建（任意登录用户可创建自己的；创建成功时只显示一次完整 token，服务端只保存它的 SHA-256 哈希，丢了只能删除重建）。用法：`Authorization: Bearer rdt_...`。权限等于创建它的账号：管理员的 token 可调管理接口，普通用户的 token 只能看到自己范围的数据。每次使用会更新 `last_used_at`。
2. **会话 Cookie（`rdp_sid`）**：浏览器登录控制台后由 `POST /api/auth/login` 下发（`Set-Cookie`，有效期 24 小时），后续请求带 `Cookie: rdp_sid=...`。网页登录要先过验证码（`GET /api/captcha` 取图，再把识别出的字符填进 `captcha` 字段），脚本调用不如直接用 `rdt_` token 省事。
3. **客户端访问令牌（`POST /api/login` 返回的 `access_token`）**：只给官方客户端协议接口用（`/api/peers`、`/api/users`、`/api/currentUser` 等）。它调不了管理接口（实测返回 401 `未登录`）；反过来 `rdt_` token 也调不了客户端协议接口（那些接口只认登录时发出的客户端令牌）。

示例约定：下面用 `TOKEN` 表示 `rdt_` token，`CTOKEN` 表示客户端访问令牌，服务器地址用 `203.0.113.10`（文档保留地址，请换成实际地址）：

```bash
TOKEN="rdt_0123456789abcdef..."   # 控制台 API Token 页创建
CTOKEN="0123456789abcdef..."      # POST /api/login 返回的 access_token
```

### 错误格式

- **管理/控制台接口**：HTTP 状态码 + `{"ok":false,"error":"..."}`。401 未登录（无凭证、凭证无效或过期）；403 需要管理员权限（或资源不属于当前用户）；400 参数错误（消息为中文说明）；404 记录不存在/链接无效；429 登录太频繁被锁定；500 服务端错误。
- **客户端协议接口**：错误也返回 HTTP 200，body 为 `{"error":"..."}`（这是官方客户端约定的格式）；只有未鉴权是 HTTP 401 `{"error":"Please login"}`。写脚本时不要只看状态码，要检查 body 里有没有 `error` 字段。

### 限流与锁定（v23 起）

- **密码登录**（网页与客户端共用同一套计数器）：同一账号 10 分钟内错 5 次 → 锁定 15 分钟起步，重复触发翻倍（15 分钟 → 30 分钟 → 1 小时……上限 12 小时，24 小时无再次触发后级别清零）；同一 IP 10 分钟内错 30 次 → 该 IP 按同样规则锁定（影响该 IP 上的所有账号）。登录成功清零账号计数器；锁定期间的尝试不会延长锁定。锁定状态在内存里，重启服务端即清零；锁定与登录失败都记到 `logs/hbbs.log`（`[auth]` 开头的行）。阈值可用 `RD_AUTH_FAIL_MAX` / `RD_AUTH_IP_FAIL_MAX` / `RD_AUTH_WINDOW_SECS` / `RD_AUTH_LOCK_SECS` 环境变量调整。
- **注册**：每 IP 每小时最多 10 次（`RD_AUTH_REG_MAX` / `RD_AUTH_REG_WINDOW_SECS` 可调）。关掉「允许开放注册」后匿名注册返回 403（零用户时仍可创建第一个管理员；已登录的管理员仍可创建账号）。
- 验证码失败不计入锁定计数。客户端登录被锁定时返回的是 200 + `{"error":"Too many failed attempts, temporarily locked. Try again in about N min"}`；网页登录被锁定时返回 429。

### 客户端协议接口（客户端访问令牌）

**客户端登录** `POST /api/login`：body 为 `{"username","password"}`，可选带 `"id"`（设备 ID）与 `"uuid"`（客户端设备标识，用于设备绑定）。成功返回：

```bash
curl -s -X POST http://203.0.113.10:21114/api/login \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"admin123"}'
```

```json
{"access_token":"ee8762dfbc822be5…（64 位十六进制）","type":"access_token",
 "user":{"name":"admin","email":"","note":"","status":1,"is_admin":true,"info":{}}}
```

账号或密码错误返回 HTTP 200 + `{"error":"Username or password is incorrect"}`。退出登录：`POST /api/logout`（带客户端令牌，服务端删除该令牌）。

**设备列表** `GET /api/peers`：返回登录账号自己的设备（管理员也只看自己的；看全部设备用下面的 `/api/devices`）。分页参数 `current`（默认 1）与 `pageSize`（默认 100，上限 1000）：

```bash
curl -s "http://203.0.113.10:21114/api/peers?current=1&pageSize=100" \
  -H "Authorization: Bearer $CTOKEN"
```

```json
{"total":1,"data":[{"id":"1234730355","user":"admin","user_name":"admin","note":"",
 "device_group_name":"","status":1,
 "info":{"device_name":"tk","os":"Windows","username":"Administrator"}}]}
```

**用户列表** `GET /api/users`：只返回登录账号本人（客户端侧栏用）：

```bash
curl -s http://203.0.113.10:21114/api/users -H "Authorization: Bearer $CTOKEN"
```

```json
{"total":1,"data":[{"name":"admin","display_name":"","avatar":"","email":"",
 "note":"","status":1,"is_admin":true}]}
```

**当前用户** `POST /api/currentUser`：返回与上面相同的单个用户对象（无 `total`/`data` 包裹）：

```bash
curl -s -X POST http://203.0.113.10:21114/api/currentUser \
  -H "Authorization: Bearer $CTOKEN"
```

**其他客户端协议接口**（官方客户端内部调用，一般不需要手工调）：`POST /api/heartbeat`（设备心跳与策略下发）、`POST /api/sysinfo`（上报设备信息）、`GET/POST /api/ab`（地址簿同步）、`GET /api/device-group/accessible`（设备组，当前返回空列表）、`GET /api/login-options` 与 `/api/oidc/*`（OIDC 浏览器登录）、`POST /api/sysinfo_ver`（返回纯文本 `1`，客户端版本协商用）。

### 管理接口（`rdt_` token 或会话 Cookie）

**当前登录信息** `GET /api/me`：

```bash
curl -s http://203.0.113.10:21114/api/me -H "Authorization: Bearer $TOKEN"
```

```json
{"is_admin":true,"ok":true,"username":"admin"}
```

**设备列表（控制台全视图）** `GET /api/devices`：管理员看全部设备，普通用户只看自己的。`last_seen` 是 Unix 秒时间戳：

```bash
curl -s http://203.0.113.10:21114/api/devices -H "Authorization: Bearer $TOKEN"
```

```json
{"devices":[{"device_id":"1234730355","uuid":"…","username":"admin",
 "hostname":"tk","os":"Windows","version":"1.5.3","last_seen":1794566574}],
 "now":1794566574,"ok":true}
```

**客户端配置** `GET /api/client-config`：任意登录用户可查，返回官方客户端手动配置需要的四个值（与控制台「官方客户端手动配置」卡片一致）；未配置公网地址（`public_host` 为空）时返回 `{"ok":true,"configured":false}`：

```bash
curl -s http://203.0.113.10:21114/api/client-config \
  -H "Authorization: Bearer $TOKEN"
```

```json
{"api":"http://203.0.113.10:21114","configured":true,"host":"203.0.113.10:21116",
 "key":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=","ok":true,
 "relay":"203.0.113.10:21117","share":"Qfi…（整份配置的分享字符串，v33 起）"}
```

（`key` 为 44 字符、以 `=` 结尾的服务端公钥，上例为占位符。`share` 是整份配置按官方客户端 `ServerConfig.encode()` 编码的字符串：四字段 JSON → base64url（无填充）→ 整体反转，与生成客户端文件名里的编码同一格式；粘贴到官方客户端「设置 → 网络」的导入处即可一次填入四项。）

**其他平台下载链接** `GET /api/client-downloads`（v33 起）：任意登录用户可查，返回最新官方客户端各平台的下载信息（与控制台「其他平台下载」区一致）。`url` 为官方 GitHub 直链，`mirrors` 为加速链接（管理员 `github_mirror` 设置优先，其次内置镜像链）；iOS 只有 App Store 直链，`mirrors` 为空。GitHub 不可达时返回 502（不编造版本号）；上游数据缓存 1 小时，镜像按当前设置每次现算：

```bash
curl -s http://203.0.113.10:21114/api/client-downloads \
  -H "Authorization: Bearer $TOKEN"
```

```json
{"ok":true,"version":"1.5.3","platforms":[
 {"file":"rustdesk-1.5.3-x86_64.dmg","id":"macos-x64","mirrors":[
  "https://gh-proxy.com/https://github.com/rustdesk/rustdesk/releases/download/1.5.3/rustdesk-1.5.3-x86_64.dmg",
  "…（其余内置镜像）"],"name":"macOS（Intel）",
  "url":"https://github.com/rustdesk/rustdesk/releases/download/1.5.3/rustdesk-1.5.3-x86_64.dmg"},
 {"file":"","id":"ios","mirrors":[],"name":"iOS",
  "url":"https://apps.apple.com/us/app/rustdesk-remote-desktop/id1581225015"}]}
```

（上例截断：实际返回 11 个平台——macOS 2 个、Linux 4 个、Android 4 个、iOS 1 个；Windows 不在其中，Windows 请用生成客户端。）

**Token 管理**：创建 `POST /api/tokens`（body `{"name":"..."}`，name 可省略，默认为 `default`），返回的 `token` 只显示这一次，请立即保存；列表 `GET /api/tokens`（只返回 id/name/时间，不返回 token 值）；删除 `DELETE /api/tokens/:id`：

```bash
curl -s -X POST http://203.0.113.10:21114/api/tokens \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"name":"backup-script"}'
# {"ok":true,"token":"rdt_…（创建时只显示一次）"}

curl -s http://203.0.113.10:21114/api/tokens -H "Authorization: Bearer $TOKEN"
# {"ok":true,"tokens":[{"id":1,"name":"backup-script",
#   "created_at":1794566574,"last_used_at":null}]}

curl -s -X DELETE http://203.0.113.10:21114/api/tokens/1 \
  -H "Authorization: Bearer $TOKEN"
# {"ok":true}，不存在的 id 返回 404 {"ok":false,"error":"Token 不存在"}
```

**生成客户端** `POST /api/admin/gen-client`：任意登录用户可生成（记录归属调用者）。body 可带 `{"label":"..."}`（备注，可省略）。需要先在控制台下载好 Windows 模板并配置公网地址，否则返回 400。返回的 `download_url` 即下载链接（见下）：

```bash
curl -s -X POST http://203.0.113.10:21114/api/admin/gen-client \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"label":"office-pc"}'
```

```json
{"download_url":"http://203.0.113.10:21114/dl/b64c2b88…/rustdesk--Qfi0DMzM…（约 120 字符的编码文件名）.exe",
 "filename":"rustdesk--Qfi0DMzM….exe","ok":true}
```

**生成记录列表** `GET /api/admin/gen-clients`：管理员看全部，普通用户只看自己的：

```bash
curl -s http://203.0.113.10:21114/api/admin/gen-clients \
  -H "Authorization: Bearer $TOKEN"
```

```json
{"clients":[{"created_at":1794566574,"created_by":"admin",
 "download_url":"http://203.0.113.10:21114/dl/b64c2b88…/rustdesk--Qfi0DMzM….exe",
 "filename":"rustdesk--Qfi0DMzM….exe","id":1,"label":"office-pc"}],"ok":true}
```

**删除生成记录** `DELETE /api/admin/gen-clients/:id`：本人或管理员可删；删除后该记录的下载链接立即失效（文件一并删除）：

```bash
curl -s -X DELETE http://203.0.113.10:21114/api/admin/gen-clients/1 \
  -H "Authorization: Bearer $TOKEN"
# {"ok":true}，不存在的 id 返回 404 {"ok":false,"error":"记录不存在"}
```

**下载生成的客户端** `GET /dl/:token/:filename`：无需登录，链接里的 token 本身就是凭证（可直接发给他人下载）。成功返回 exe 文件流（`Content-Type: application/octet-stream`，`Content-Disposition: attachment`）；链接无效或记录已删除时返回 404 `{"ok":false,"error":"下载链接无效或已删除"}`：

```bash
curl -O "http://203.0.113.10:21114/dl/b64c2b88…/rustdesk--Qfi0DMzM….exe"
```

**在线更新（v32 起，仅管理员）**：

```bash
# 当前版本与暂存状态
curl -s http://203.0.113.10:21114/api/admin/update/status \
  -H "Authorization: Bearer $TOKEN"
# {"current":"v32","current_num":32,"downloaded":0,"error":"","ok":true,
#  "platform":"linux-x86_64-glibc","running":false,"source":"",
#  "staged":false,"staged_version":null,"state":"idle","total":0,
#  "upstream_base":"1.1.17","version":""}
# state: idle / downloading / extracting / done / error；
# 下载中时 downloaded/total 为已下载/总字节数，source 为当前下载源

# 检测更新（直连 API → 镜像 API → 重定向取版本号，全部失败才报 502）
curl -s -X POST http://203.0.113.10:21114/api/admin/update/check \
  -H "Authorization: Bearer $TOKEN"
# {"has_update":false,"latest":"v31","notes_unavailable":false,"ok":true,
#  "published_at":"2026-11-13T09:31:31Z","release_notes":"# v31\n\n## 修复\n\n…（完整 body）"}
# 只有版本号、拿不到日志时 notes_unavailable 为 true，release_notes 为空

# 下载并暂存指定版本（version 必须是 v33 这样的格式；立即返回，用 status 轮询进度）
curl -s -X POST http://203.0.113.10:21114/api/admin/update/download \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"version":"v33"}'
# {"ok":true,"started":true}（已有下载在进行时 started 为 false）

# 重启生效（仅在已暂存时可用；无暂存返回 400）
curl -s -X POST http://203.0.113.10:21114/api/admin/update/restart \
  -H "Authorization: Bearer $TOKEN"
# {"ok":true}，服务端约 0.5 秒后退出，由启动器换上新版并拉起
```

**其他管理接口**（控制台页面内部使用）：`GET /api/admin/overview`（概览统计）、`GET /api/admin/users` 与 `POST /api/admin/users`（新建用户）、`POST /api/admin/users/:name/password`、`POST /api/admin/users/:name/:action`（用户管理）、`POST /api/auth/change-password`（自助改密码，任意登录用户）、`GET/POST /api/admin/settings`（服务器设置）、`POST /api/admin/template/download` 与 `GET /api/admin/template/status`（模板下载）。这几个接口的字段以控制台页面的实际调用为准，不再逐一列出。

## 日志

- hbbs / hbbr 运行时同时输出到终端和文件：程序目录下自动生成
  `logs/hbbs.log`、`logs/hbbr.log`（追加写入，不会覆盖历史）。
- API 的 SQL 查询日志已关闭，日志里只剩事件级信息。排查连接问题
  时常用：客户端完成注册看 `update_pk <设备ID>`；注册被拒看
  `uuid mismatch`（客户端身份与库中记录不一致）或 `too frequent`
  （同一 IP 注册过于频繁被临时拦截）；中继配对过程看 hbbr 日志。
- 需要更详细输出时用环境变量启动：`RUST_LOG=debug ./hbbs`
  （`trace` 更细，会非常吵）。

## 安全建议

- 21114 默认是 HTTP，v23 起按顺序做这几件（详见上方 v23 一节）：
  ① 管理员用强密码，并在服务器设置里关掉「允许开放注册」（登录
  已有限速锁定与审计日志兜底）；② 在 server.env 设置 CONSOLE_LISTEN
  把网页控制台移出公网；③ 有域名时用 TLS_CERT/TLS_KEY 开启原生
  HTTPS（须受信任证书）。
- 也可以用 Nginx/Caddy 反代终止 HTTPS，但反代后服务端看到的来源
  IP 是反代机地址，IP 维度的锁定会按反代地址聚合，请知悉。

## 许可

RustDesk 为 **AGPL-3.0** 许可。本改造同样遵循 AGPL-3.0：
自用/内部部署没有问题；若对外分发或商业销售，需要履行 AGPL 的
源码提供义务。本仓库保留了上游全部源码与许可文件。

## 已验证与未验证

已在本机通过自动化联调（33 项）：账号注册/登录、官方客户端登录
契约（密码登录 + OIDC 浏览器登录全流程）、心跳/设备绑定、策略
下发、用户管理、Token、模板下载、生成客户端与下载链接。

未验证：官方真实客户端在本服务上的端到端登录与远控画面——需要在
真实机器上用「生成客户端」产出的安装包做一次冒烟（安装 → 登录
账号 → 出现在后台设备列表 → 发起一次远控）。
