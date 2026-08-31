# Marol

[English](README.md) · **繁體中文**

多個 coding agent session 的桌面管控台。**每個 session 就是一個真的終端機**，
跑真的 `claude`（或 codex / gemini / aider），畫面跟你在 Terminal.app 裡開的
一模一樣：同樣的 TUI、同樣的 `/` 選單、同樣的權限提示。App 不重繪、不重新
詮釋任何東西。

它補上的是終端機分頁給不了的東西。每張卡片有自己的 git worktree，所以同一個
repo 上的兩個 agent 不會互相踩到。每個 session 會回報自己是不是卡在你身上，
所以唯一值得知道的那個數字就在畫面上。而整件事跑在跟你的 shell 一樣的環境
裡，所以 agent 找得到的工具跟你一樣。

![看板：agent 分佈在整個生命週期，執行中、等你、可合併、已合併、擱置](docs/media/board.zh.png)

---

## 導覽，一個功能一支短片

### 分流

某張卡轉成琥珀色。桌上不會有別的東西這樣跳動，所以不需要閱讀。
`⌘/Ctrl+E` 把你放進那個 session 的終端機，游標已經在裡面。

![卡片轉成琥珀色，一顆鍵就落進它的終端機](docs/media/clips/zh/triage.gif)

### 開卡

`⌘/Ctrl+K` 打開時，等你的 session 已經列在那裡，你一個字都還沒打：
它首先是待辦收件匣，其次才是搜尋框。改成打一句話，它就變成一張卡。

![命令面板把打進去的一句話變成一張卡](docs/media/clips/zh/compose.gif)

### 開始 attempt

對話框攤開組好的完整 prompt 而且可以改，所以跑的就是你讀過的。權限模式在
這裡決定，只作用於這一次 attempt。開始之後會開一個隔離的 worktree，和一個
真的終端機。

![開始對話框、組好的 prompt、以及它開出來的終端機](docs/media/clips/zh/attempt.gif)

### 檢視

diff 開在活著的終端機**旁邊**，不是取代它。點某一行附上意見，整批走 session
自己的輸入送回去，所以 agent 收到的是一份 review，不是一串碎片。

![對 diff 的某一行留言，整批送回 agent](docs/media/clips/zh/review.gif)

### 自己在 diff 裡改掉

review 迴圈最常見的收尾是一行小修，所以 diff 直接讓你修。`✎` 把檔案就地
展開成編輯器，存檔寫進 attempt 的 worktree，接著遞上一則寫明檔名的
「告訴 agent」訊息。

![就地編輯、存檔，以及跟在後面的那則訊息](docs/media/clips/zh/edit.gif)

### 規則檔

在任何人打字之前 agent 就已經讀過的東西：規則檔與 skills，來自這份 checkout
與這台機器。不存在的也列出來，因為「這裡沒有 CLAUDE.md」正是你來找的答案。

![「規則檔」分頁列出規則與 skills，有的和沒有的都列](docs/media/clips/zh/knows.gif)

### 設定

用它在畫面上叫什麼去搜，而不是記得它在第幾層抽屜。刻意不存在的設定，就在
你找它的地方說明為什麼不存在。

![搜尋設定面板](docs/media/clips/zh/settings.gif)

---

## 其他畫面

<details>
<summary>總覽、命令面板、檢視器、時間軸、終端機牆</summary>

**總覽。** 所有 session 一次看完，依它需要你做什麼分組；超過一台機器時再按
機器分開。

![總覽](docs/media/overview.zh.png)

**命令面板。** 等你的最前，然後是完成未看的，再來是卡片與動作。

![命令面板](docs/media/palette.zh.png)

**檢視器。** diff 就地展開成編輯器（base 唯讀嵌在行間、worktree 側可改）、
這個 attempt 的 token 帳、一則待送的 review 留言，以及點擊前就先跑好的合併
檢查。

![檢視器抽屜，可編輯 diff 展開中](docs/media/inspector.zh.png)

**活動與檢查點。** agent 做了什麼，按工具摺疊起來，每一輪之前的等待都標出
花了多久。每個 prompt 列都戴 `↩`，把程式碼還原到那一輪之前。

![活動時間軸與檢查點](docs/media/timeline.zh.png)

**終端機牆。** 每個 session 都是真的 PTY：真的 Claude Code TUI 一個像素不差，
旁邊是一般的 test runner。

![兩個真終端機並排](docs/media/wall.zh.png)

**設定。** 分區、搜尋，以及那些拒絕。

![設定面板](docs/media/settings.zh.png)

</details>

---

## 裡面有什麼

- PTY session：真的 pseudo-terminal 跑真的 agent CLI，xterm.js 渲染
- login-shell 環境解析，agent 拿到的 PATH 跟你終端機一樣
- SQLite session 清單，跨重啟保留；重開會 `--continue` 接續該目錄的對話
- 多個工作區分頁，各自保留自己的佈局與 scrollback
- 任意 agent CLI 加任意啟動參數，原封不動傳過去
- **狀態偵測與通知**：靠 agent 自己的 hooks，不解析 ANSI。左上角顯示
  「⚠ N 個等你」，被擋住的 session 會發系統通知。兩支實測過的 CLI ——
  Claude Code 與 Codex —— 回報同樣的六個時刻（見「兩支實測過的 agent」）
- **任務與 attempt**：一張卡可以開多個 attempt，每個有自己的 git worktree
  與分支，同一個 repo 上的兩個 agent 互不干擾。收尾時 diff 先凍結進資料庫，
  再把 worktree 還回去
- **一張卡可以跨多個 repo**：一個要同時改後端和它的客戶端的變更，是一張卡、
  一段對話。每個 repo 各開自己的 worktree、用同一個分支名，並排在同一個
  資料夾裡，agent 就起在那個資料夾。diff、審查、合併全部涵蓋
- **看板**：四欄、卡片可拖曳。卡片帶著自己的即時狀態，所以一張待在「進行中」
  的卡可以亮起「⚠ 等你授權」，點下去直接進那個 session 的 TUI。每張卡片一樣
  高，所以卡片在你眼前變化時，看板仍然掃得動。沒有卡片的 session 也落在同樣
  的欄位裡，依它正在做什麼排：活著的進「進行中」，關掉的進「已完成」，邊緣
  是虛線 —— 它沒有 worktree，也沒有東西可合併
- **變更與活動**：TUI 旁邊的抽屜，不進終端機就能說出這個 attempt 改了什麼
  （含未 commit 與新建檔）、做了什麼
- **收尾與併發**：合併回 base、push 並開 PR、丟棄。同時執行數有上限（預設
  3），超過的卡片排隊，額度一放出來自己起跑
- **檢視迴圈**：在 diff 裡點某一行、附上意見，整批一次送回還開著的 session，
  走 session 自己的終端機（bracketed paste），所以多行意見是**一則**訊息，
  時間軸也記下實際問了什麼。沒實測過輸入慣例的 CLI 拿到的是「複製」而不是
  「送出」，跟首則 prompt 同一套誠實。合併某個 attempt 時，同卡其他還開著的
  attempt 自動標為已被取代，diff 凍結保留，方便事後比較兩個 agent 的做法
- **Workspace scripts**：新開的 worktree 只是個 checkout，不是能跑的工作區。
  `.marol/config.json` 說明它怎麼長成一個（見下）
- **權限模式**：每個 attempt 可以選實測過的 CLI 要照常詢問、自動接受檔案
  編輯，或全自動不再詢問。這句話在命令列上怎麼寫是那支 CLI 自己的事 ——
  Claude Code 有 permission mode（`--permission-mode acceptEdits`、
  `--dangerously-skip-permissions`），Codex 有沙箱與核准政策
  （`--sandbox workspace-write --ask-for-approval on-request`、
  `--dangerously-bypass-approvals-and-sandbox`）—— 這張桌子存的是人核准了
  什麼，不替任何 agent 把設定翻譯成另一個 agent 的。安全論證就是 worktree，
  所以這個選項
  只存在於 attempt，沒有卡片的 session 永遠沒有。模式在建立對話框核准一次，排隊與
  resume 都會沿用；session 全自動跑著的時候，卡片一直掛著 ⚡ 徽章
- **具名設定檔**：profile 就是幫「這個 CLI、每次都帶這些參數」取一個名字，
  例如 `opus 版` 代表 `claude --model opus`。記錄與 resume 用的都是底下真正
  的 CLI，所以 prompt 遞送、狀態 hooks、權限模式全部照實際跑的東西判斷
- **跨 session 互傳訊息，用卡片名字**：Claude Code v2.1.224+ 讓同一台機器上
  的 session 可以互傳訊息，而 Marol 的每個 session 都是真的 `claude`，
  所以卡片之間本來就通。桌面補上的是名字。CLI 自己會用 worktree 目錄名幫
  session 取名，一串 slug 加編號，所以 Marol 改用 `--name` 把 session
  命名成它自己的標題，於是一張卡的 agent 可以用「修好登入 #1」這種人會說出
  口的名字去找另一張卡的 agent。送出的訊息會落在活動時間軸上。啟動時探測
  一次 `claude --version` 做版本閘門，因為舊版 CLI 遇到不認識的 flag 會直接
  拒絕啟動
- **名字可以改，session 也可以自己取名**：有卡片的 session 叫卡片的名字；
  沒有卡片的終端機以前只能叫它開在哪個目錄，所以同一個 checkout 裡開幾個
  就有幾列寫著同一個字。現在它們會自己往上數（`repo`、`repo 2`），更重要的
  是可以改名：在側欄或總覽上雙擊那一列、按 F2，或按 ✎。session 裡的 agent
  也可以自己設——它的 plugin 帶一個 skill，而 `$MAROL_NAME_URL` 就是這個
  session 在狀態 hooks 本來就在用的那個 listener 上的位址，所以
  `curl -X POST "$MAROL_NAME_URL" --data-binary "改登入導向"` 就是全部了。
  改名立刻反映在桌面上；至於別的 session 拿來傳訊息的那個 `--name`，它釘在
  一條已經跑起來的命令列上，所以要等這個 session 下次啟動才會換
- **活得比 app 久的 session**：agent 的 session 由 `tmux` 扛著，一個 session
  一個 socket，而且是在它自己所在的世界裡——本機、WSL distro、SSH host 都算。
  關掉 Marol 是斷開，不是殺掉。重開卡片會接回那個一直在跑的 agent（見下）
- **WSL 橋接**：卡片的 repo 可以住在 WSL distro 裡，一切就在 repo 所在的
  世界執行
- **SSH host**：同一道接縫，跨一條線，用的是你自己 `~/.ssh/config` 裡的
  `Host` 別名
- **系統匣圖示**：等待數，以及視窗關掉之後回去的路。它主要是為 Windows 存在的，
  因為那裡根本沒有 dock 徽章
- **中英雙語**：跟隨系統語言，也可以在設定裡手動切換。系統原生通知與系統匣選單
  都會跟著換

---

## 活得比 app 久的 session

agent session 跑在 `tmux` 裡，一個 session 一個 socket。關掉 Marol 只是
斷開 client，agent 繼續跑。重開卡片接回的是那個從來沒停過的行程，包括做到
一半的那一輪。

五個值得點名的判定：

- **`new-session -A -D` 就是 create-or-attach**，所以「第一次開」和「重啟後
  接回」是同一條程式碼路徑，兩者不可能對不上。
- **退出 app 是斷開；關閉 session 才是銷毀。** 這個區別就是整個功能：退出
  不等於做完了。
- **只有 agent 的 session 被扛住。** run script 與 worktree shell 是你開來看
  的，跟著桌子收掉；agent 是你開來讓它跑的。
- **socket 名字帶著每個安裝自己的標籤**（data 目錄的 FNV-1a），所以一個安裝
  的孤兒清掃永遠不會殺掉另一個安裝的活 agent。
- **持久化是世界的能力，不是 app 的前提。** 有 `tmux` 的世界就有，沒有的世界
  保持它原本的行為一模一樣——而一個剛裝好的 WSL Ubuntu 就是沒有。不會有任何
  東西被替你裝上去，本機或遠端都一樣。

### 每一個世界，不只是這一台

WSL distro 與 SSH host 也是同一回事，而唯一需要改的只有 socket 怎麼命名。
`-L <名字>` 是去問 `tmux`「你的 socket 目錄在哪」，而這個問題只有本機答得出來：
在別的世界，那個目錄取決於一個這邊看不見的 uid 與 profile，於是一次猜錯的清掃
會看進一個空目錄，然後判定所有還活著的 agent 都死了。所以在別的世界改由 app
自己指定路徑——`~/.marol/s/`——再用 `-S` 告訴 `tmux`。本機維持 `-L`，因為
換掉會讓舊版本留下、還在跑的每一個 session 卡在一個再也沒人會去找的名字底下。

從這一個改動長出來的三件事：

- **設定檔要送進那個世界。** `-f` 指到一個不存在的檔案時 `tmux` 不會抱怨，它
  會用預設值啟動，然後在 agent 的終端機上畫一條狀態列。所以寫不進去的時候是
  「這個 session 不被扛住」，而不是「被一個會重繪的 tmux 扛住」。
- **在外面，socket 名字還要帶上機器的身分。** 同一個人的兩台筆電有同一個 data
  目錄；如果兩台都連到同一個 SSH host，它們會算出同一個標籤，然後其中一張桌子
  的孤兒清掃會無聲地殺掉另一張桌子正在跑的工作。寫進 data 目錄一次的隨機 id
  就是把它們分開的東西。
- **結束遠端 session 時，socket 檔要在同一個命令裡刪掉。** 沒有第二次機會：
  這個行程碰不到那個檔案系統，而 `tmux` 的 server 結束時會把 inode 留在原地，
  於是下一次清掃看到的殘檔和活著的 server 長得一模一樣。

被扛住的 session 回來時叫做 **執行中，尚未回報**——它在跑，而且目前也就只知道
這麼多，所以圓點用中性色。本機是在第一次繪製之前用 `tmux has-session` 問過的，
不是丟給背景執行緒：一個過一下才自己修正的狀態，在一個唯一職責就是「一眼可信」
的表面上是閃爍。其他世界則是丟到執行緒上問，因為問之前得先探測那個世界——一個
login shell，SSH 的話還要一條連線——而「畫面要等一台筆電跟伺服器講完話才肯出現」
是兩者裡比較糟的那個。至於答不出來的世界，什麼都不動：連不上不等於不在了。

但它不會一直停在那裡。**hook 的 endpoint 跨重啟是同一個**：port 按號碼再要
一次、token 留著，所以那個 session 的 plugin 設定裡烤進去的 URL 仍然打得到，
agent 的下一個事件就會把真的狀態放回那一列。要「endpoint 穩定」而不是「URL
做間接層」，正是因為它被烤進去了：除了 `SessionStart` 之外全都是 `http`
hook，`url` 是一個死字串、後面沒有 shell，而 Claude Code 只在 session 開始時
讀一次那個檔。對一個已經在跑的 session 來說，那個檔是照片，不是指標。

兩個值得點名的後果：

- **重新接上不等於啟動。** `new-session -A -D` 是接回正在跑的 agent，argv 直接
  丟掉，所以不會再有 `SessionStart`。在那裡宣稱「啟動中」，就是狀態標籤以前說
  過的同一個謊、從另一面再說一次，而且永遠不會自己修正。
- **如果那個 port 被占走了**（另一個 Marol，或任何別的程式），就換一個新
  的，而上一輪留下的 session 會安靜到它自己結束為止。那正是「還沒有記住
  endpoint」之前的狀態，所以它是降級，不是拒絕啟動。

SSH host 是透過反向隧道打回那個 listener 的，所以它還有第二個 port、同一個
問題，答案也一樣：遠端 port 按 host 記下來，記不到的時候就從 host 名字加上這
台機器的 id 推出來。兩半都重要——host，是為了讓同一張桌子的兩台伺服器不撞在
一起；機器，是因為那個 port 綁在**遠端**那一側，兩台筆電連到同一台伺服器時
不然會跟它要同一個號碼。而 `ssh -f` 是認證完就 fork，就算 forward 被拒絕也照樣
回 0、只把訊息印在沒人會看的 stderr 上，所以這裡開了 `ExitOnForwardFailure`：
被拒絕就是一個答案，然後換下一個號碼試。

---

## 系統匣

一個會說「有沒有東西在等你」的圖示，以及視窗不在時回去的路。平台畫得出標籤的
地方就在圖示旁邊寫 `⚠ 3`，畫不出來的地方用文字寫在 hover 上，而沒有東西在等的
時候什麼都不寫：一個永遠掛著自己名字的系統匣，是拿選單列的一塊常駐空間去講一
件你已經知道的事。

它主要是為 Windows 存在的。macOS 與 Unity 本來就把等待數畫在 dock 圖示上，所以
在那裡系統匣是把說過的話再說一次；Windows 沒有徽章，而在這之前，關掉視窗等於
「有 agent 被擋住」這件事完全沒有任何訊號。

三件它刻意不做的事：

- **關視窗仍然是你的平台說的那個意思。** 把「關閉」偷換成「隱藏」是系統匣應用
  常做的事，而它會嚇到每一個真的想結束的人。何況現在比以前更不需要這樣做了：
  結束變便宜了，agent 活得比它久。
- **從系統匣結束，就是同一個結束。** 它走的是跟其他所有結束一樣的離開路徑，所以
  被 tmux 扛住的 session 是斷開而不是變孤兒，hook 的 port 也會還回去給下一輪。
- **選單不列出正在等你的 session 名字。** 那是個真的想法，而且是個更大的：它需要
  在每次狀態改變時重建清單，還需要一條點回 webview 的路。而系統匣存在要回答的
  那個問題，也就是「要不要現在過去看」，數字已經回答了。

---

## 讓 worktree 開箱能跑

在 repo 放一個 `.marol/config.json`，每個 attempt 的 worktree 就會自己
準備好：

```json
{
  "setup": "npm install && cp \"$MAROL_ROOT_PATH/.env\" .env",
  "run": [
    { "name": "dev", "command": "npm run dev -- --port $MAROL_PORT" },
    { "name": "test", "command": "npm test -- --watch" }
  ],
  "archive": "docker compose down"
}
```

`setup` 在 agent 起跑前執行，跑在同一個終端機裡，所以輸出跟失敗都在你正在
看的地方。`run` 的每一項變成抽屜裡的 ▶ 按鈕，在該 attempt 自己的 worktree
裡開 dev server 或 test watcher，`$MAROL_PORT` 帶一個沒人占用的埠。
`archive` 在 worktree 被收回之前執行。每個 script 都看得到
`$MAROL_ROOT_PATH`，也就是 worktree 是從哪個 repo 開出來的，`.env` 這類
沒進版控但值得複製的檔案就在那。

一張卡跨多個 repo 時，每個 repo 自己的設定檔都算數，**各在各自的 checkout
裡**：

- `setup` 依卡片上的順序串成一支腳本，每一段在自己的 checkout 裡跑，
  `$MAROL_ROOT_PATH` 也逐段指向自己那個 repo —— 所以
  `cp "$MAROL_ROOT_PATH/.env" .env` 會把客戶端的 env 放進客戶端、後端的放進
  後端。`set -e` 照樣讓整串停在第一個失敗，停在你面前。（agent 自己的行程
  繼承的是**第一個** repo 的 `$MAROL_ROOT_PATH`。）
- `run` 的名字帶上它屬於哪個 checkout —— `web:dev`、`api:dev` —— 因為兩顆都
  寫著 `dev` 的按鈕是兩顆沒人分得出來的按鈕；按下去也起在那個 checkout 裡，
  它自己的 `package.json` 所在的地方
- `archive` 逐 repo 執行，各在各自的 checkout 裡，在那棵 checkout 被收回之前

Script 都走 `sh -c`，寫法跟在終端機打一行一樣。檔案格式錯誤會讓 attempt 在
對話框裡就開不起來，而不是安靜地什麼都不做：一個安靜失效的設定檔，跟一個
壞掉的 worktree 從外面看是分不出來的。（目前僅支援 POSIX 平台。）

`.agentdesk/config.json` 與 `$AGENTDESK_*` 仍然有效，而且會一直有效。這個檔案
是這次改名唯一動到、但不屬於這個 app 的東西：它住在**你的** repo 裡，通常是進
版控的，而你的協作者不一定在跑這張桌子。兩組變數名指向同一個值，所以你想什麼
時候把 repo 換過來都行，不換也行。

---

## 執行

需要 Node 20+、Rust stable，以及你要用的 agent CLI 已安裝並登入。

```bash
npm run setup
npm --prefix ui run dev &                       # vite on :5173
cargo run --manifest-path src-tauri/Cargo.toml
```

`cargo` 若不在 PATH，先 `source ~/.cargo/env`。要永久生效，把這行加進
`~/.zshrc`：

```sh
export PATH="$HOME/.cargo/bin:$PATH"
```

### 迴圈周邊

撐起 triage 迴圈的各個部件，大致依你遇到它們的順序：

- **首次啟動** 會落在看板上，空的 backlog 欄就是入口。歡迎面板回報這台機器
  實際裝了哪些 agent CLI，並用看板自己的點形畫成三點軌，講完卡片 →
  attempt → 收尾這條模型。PATH 上一個 agent CLI 都沒有的機器，拿到的是誠實
  的琥珀色死路加一顆「重新偵測」chip，不是一片開朗的空白。面板隨時能從命令
  面板或設定重開，重開就重新探測，不重播舊答案。之後五個一次性的 coach mark
  會在某個介面第一次派上用場時指出它，然後永遠閉嘴。第五個教的是琥珀色呼吸
  與 `⌘/Ctrl+E`，出現在第一次有 session 從「在做」轉成「等你」的時候。第一個
  session 開起來之前，空的終端機牆顯示一張三列的按鍵卡，不留白。
- **未讀層。** session 在終端機不在你眼前時完成一輪，側邊欄、分頁徽章、總覽
  都會掛上未讀點，直到那個終端機真的出現在螢幕上。
- **側欄分區。** 等你（與 ⚠ 徽章數的是同一批）最上，然後開發中、待命、已完成。
  待命獨立成區：一輪結束是輪到你，但沒有東西卡在你身上。
- **看板即時預覽。** 選中卡片就在欄位旁邊顯示它真正的終端機，進去之前唯讀，
  所以「它現在到底在幹嘛」只花一次點擊，不用切頁。
- **檢視器**（`⌘/Ctrl+I`）。attempt 的 diff 有逐檔已讀、換行、檔案跳轉、可調寬
  的抽屜；時間軸把同工具連跑摺疊起來、標出每次等待花了多久；shell 分頁在
  attempt 的 worktree 裡開一個真的終端機；還有從 git 讀出來的下一步建議，只在
  輪得到人做決定時出現。
- **佇列 follow-up。** agent 回合進行中寫的回饋會先扣住，回合結束後合成一則
  送出。banner 會寫明佇列裡有什麼，一鍵取消。
- **檢查點。** 有便宜的退路，才敢放手讓 agent 跑：最壞的結果只值一次點擊時，
  讓它自己做完就容易得多。每輪結束把 worktree 快照進私有 ref（預設開，設定
  裡可關），agent 看得到的一切原封不動；另有 ⚑ 手動快照，任何 agent 都能用。
  時間軸的 prompt 列戴 `↩`：把程式碼還原到此輪之前。對話永不觸碰、還原前先
  自動快照、回合進行中會拒絕並說明理由。diff 可改以任一檢查點為基準比較。
  refs 隨 attempt 終局刪除，凍結 diff 從此是唯一紀錄。
- **擱置。**「現在不做」不等於「不做了」：對安靜下來的 attempt 按擱置，
  worktree 與併發槽還回去，分支、檢查點、對話全部留著（分支名同時進剪貼簿）。
  繼續時 worktree 在原路徑長回來、擱置時的工作原樣還原，`--continue` 接上原本
  的對話。
- **一張卡一個大聲的動作。** 停下來的卡片只讓「繼續」大聲，擱置與換 agent 要
  對準了才現身；合併完的卡片上「再試一次」不再壓過它底下那場勝利。檢視器的
  五顆工具 chip 也因為同一個理由收成一條有標籤的 worktree 帶：五個一樣大聲，
  等於沒有層次。
- **Dev server 預覽。** ▶ run script 起的頁面直接掛在桌邊：iframe 顯示的就是
  server 送出的樣子，不代理、不注入。server 死了面板會說，不留白框。repo 自掛
  inspect script（`docs/examples/marol-inspect.js`）後，Alt+click 任何元件
  就變成「{component} · {file}:{line}」，一鍵送進 agent 的終端機。
- **Token 帳。** 每個實測過的 session 的花費與 context 水位，每回合結束從它自己的
  transcript 讀一次（路徑由 hooks 遞來，回合中不輪詢）。檢視器顯示
  「context 279k · 輸出 2.6M」，hover 給四欄精確值。只給 token、不給金額或百分比：
  價目表會過期，沒量測過的 context window 是發明出來的分母。兩支 CLI 記帳的
  方式不同 —— Claude Code 一則訊息一列，Codex 每列都是累計總和 —— 所以摺疊
  的方式也不同：把 Codex 的列加起來，會讓一個 session 的帳單乘上它的回合數。
- **終端機搜尋**（`⌘/Ctrl+F`）。用浮層搜 10k 行捲動歷史；Enter 與 Shift+Enter
  走上下一個，找不到會直說。終端機內改用 Ctrl+Shift+F，因為 Ctrl+F 屬於
  readline。輸出裡的網址用 ⌘/Ctrl+click 開啟。
- **分支挑選。** 開卡對話框直接建議 repo 的分支、按最近使用排序，而不是要你
  憑記憶打字。標題可以不填：留白就用 prompt 的第一行，這是對話框明講的規則，
  不是碰巧的預設。
- **資料夾挑選。** 自己畫的，不是系統的那個。因為系統的對話框看的是 app 正在
  跑的那台機器 —— 對一張 WSL 卡來說那是 Windows 側，只能靠導航到
  `\\wsl$\<distro>` 穿過檔案總管才勉強到得了；對一台 SSH 主機來說，那個檔案
  系統根本沒有掛載。所以改成問那個世界：一份清單，走的是其他每件事都走的同一
  道門，local、WSL、SSH 一模一樣。它開在那個世界自己的 home，輸入框吃整條路徑
  給已經知道要去哪的人，方向鍵和 Enter 走得動，而且一個目錄如果本身就是 git
  checkout，它會在那裡就說，不用你進去才發現。
- **主機。** 左下角的切換決定新東西開在哪（WSL distro 與 SSH host 用枚舉的，
  不發明），點選就地探那台主機有哪些 agent。走 WSL 或 SSH 的 repo 會在卡片上
  戴 host 徽章；總覽在超過一台主機時按機器分組。
- **無訊號 chip。** 狀態來自 agent 自己的 hooks；跑沒有 hooks 的 CLI 的卡片
  會直說「沒有狀態回報」，不讓安靜被讀成沒事。

### 鍵盤

最高頻的迴圈，agent 等你、你授權、繼續下一個，不用碰滑鼠。`⌘/Ctrl+/` 會在
app 裡顯示這份清單：

| 按鍵 | 動作 |
|---|---|
| `⌘/Ctrl+E` | 在等你的 session 之間循環 |
| `⌘/Ctrl+K` | 命令面板：等你的 session 最前，然後是卡片與動作 |
| `⌘/Ctrl+Shift+N` | 直接開新卡對話框 |
| `⌘/Ctrl+Enter` | 送出打開中的建立對話框。IME 選字的 Enter 絕不誤送 |
| `⌘/Ctrl+1` `2` `3` | 終端機牆 · 看板 · 總覽 |
| `⌘/Ctrl+Alt+←` `→` | 聚焦下一個 / 上一個 pane |
| `⌘/Ctrl+←` `→` `↑` `↓` | 搬動聚焦的卡片：左右換欄、上下換位 |
| `Ctrl+PgDn` `PgUp` | 下一個 / 上一個分頁 |
| `⌘/Ctrl+I` | 開關檢視器 |
| `⌘/Ctrl+B` | 把側欄收成一條軌，再把它叫回來 |
| `⌘/Ctrl+,` | 設定 |
| `J` `K` | 在 diff 行之間移動；`Enter` 對該行留言 |
| `N` `P` | 在 diff 檔案之間移動；停在檔頭時 `e` 就地展開編輯器、`v` 標該檔已看 |
| `Esc` | 關閉打開的對話框 |
| `Tab` `Enter` | session 列、看板卡片、diff 行都可聚焦；Enter 執行 |

在終端機裡，app 的快捷鍵要多按 Shift，也就是 `Ctrl+Shift+E` 而不是
`Ctrl+E`，就像 `Ctrl+Shift+C` 是複製一樣。那裡的 `Ctrl+字母` 屬於 shell。

收起來之後，側欄留下的是一條軌而不是什麼都沒有。只有兩樣東西活過這次摺疊：
回去的路，因為一個只能用快捷鍵離開的狀態會把人關在裡面；還有「幾個在等你」，
因為那是這張桌子存在的理由。列本身是真的卸載掉的 —— 那是重點不是副作用，
驅動它們計秒的那個一秒一次的計時器也跟著走。

快捷鍵表另外用一張表列出屬於 **agent** 而不屬於 Marol 的鍵：Codex 的
`Ctrl+T` 打開它自己的 transcript，pager 鍵在裡面移動。分開列是因為 Marol
改不動它們，混在同一張表裡等於宣稱改得動。

已輸入文字的對話框會忽略誤點 backdrop（Escape 仍然關得掉）；刪除卡片要按
兩下，第二下會用文字說明它要做什麼。

焦點用交的，不用丟的：命令面板落在它點名的那張卡上；開新卡會切到看板、聚焦
新卡並唸出來；從空的終端機牆合併，焦點落在剛裁決完的卡片上；送出 review
批次之後，游標交還給 diff。

### 捲動一個整頁的 agent

滑鼠滾一格會發生什麼，由畫面上那個 agent 決定，只有三種。在 normal buffer
上它捲動這個 pane 自己的 10k scrollback，一個 byte 都不會送給程式。開了
mouse tracking 就變成一則 mouse report，由程式自己捲。在 **alternate**
buffer 上沒有 scrollback 可捲，所以 xterm.js 把滾輪轉成方向鍵、讓程式自己
處理 —— 而每一個被 tmux 握住的 agent 都住在那裡，因為 tmux 一 attach 就送
`smcup`。

想法是對的，xterm.js 的算術不對。它算出一格值幾行，然後只送一行；而且把小於
50px 的 pixel delta 當「大概是觸控板」乘以 0.3 再向下取整到整格 —— 以 ~17px
的 cell 配觸控板實際送出的 ~4px delta 計算，十四次裡大約十三次什麼都不送。
在筆電上這不是邊緣情況，感覺就是滾輪壞了。

所以算術改成 Marol 自己的：不足一行的 delta 跨事件累積，直到夠一行為止；一格
送它該送的行數。兩種情況原封交還給 xterm.js —— 自己要了 wheel report 的程式
擁有自己的滾輪，而 normal buffer 有真的 scrollback，該動的是 viewport。tmux
那邊什麼都沒改：`set -g mouse on` 評估過後否決，因為在 alternate screen 上
tmux 自己的綁定本來就是轉發，買不到東西，卻要賠掉「tmux 永不畫任何一格」
這個承諾。

### 螢幕閱讀器

終端機用 GPU（WebGL）渲染，畫出來的是螢幕閱讀器讀不到的像素。設定裡有一個
可選的終端機螢幕閱讀器模式，用 DOM 渲染器把它換掉：終端機文字（含權限提示）
變得可讀，代價是大量輸出時捲動沒那麼順。這筆交換就寫在設定自己的說明文字裡，
因為宣稱無代價的無障礙模式，必然對其中一邊說謊。

周邊會說話的部分：卡片的標籤唸得出權限模式，全自動跑的 session 絕不會被聽成
有人看著的那種；回合結束透過 live region 播報；每顆圖形按鈕都有真名字；分隔
軸把真實數值交給輔助技術；世界選單可以用方向鍵走。

### 通知

session 開始等你（權限確認、資料夾信任）而視窗不在前景時，OS 會用 app 的語言
跳出通知，dock 或工作列圖示會掛上等待數（macOS 與 Linux）。視窗在前景時 app
內的 banner 已經說了，OS 就保持安靜。

設定裡可以選哪些類別要發：授權與信任確認、等你回覆、完成一輪。還有一顆測試
按鈕，因為發現通知設定壞掉的那一刻，不該是 agent 已經卡住的那一刻。

### 主題

五個預設主題：墨（預設）、紙（淺色）、松、紫藤、落日，加上自訂模式。自訂主題
只問六個真正載義的顏色（背景、文字、強調色、成功/警告/錯誤），中間的層次自動
推導。編輯器會即時顯示每一層文字對照它實際所在底色的 WCAG 對比，4.5:1 是 app
對自己保持的樓地板。終端機跟著主題換裝，淺色主題用淺色 ANSI 色盤。選擇存在
本機。

---

## 測試

```bash
cd src-tauri && cargo test      # PTY、hooks、worktree、attempt、timeline、queue、migration、規則、儲存
npm --prefix ui run test:e2e    # Playwright：前端 + 看板 + 檢視抽屜 + 佇列 + xterm 渲染 + journeys
```

macOS 的 WKWebView 沒有 WebDriver，所以 Playwright 是在 Chromium 裡跑同一份
React 樹、搭配 mock 的 Tauri IPC。它涵蓋 IPC 邊界以上的一切：session 清單、開新
session 的流程，以及 xterm 對**真實 PTY bytes** 的解碼與渲染。

測試驗的是會決定體驗真偽的性質，不是「有沒有輸出」：

- `tests/pty.rs`：子行程在 tty 上（所以 CLI 進互動模式，不是降級的
  non-interactive），以及它拿到的是 login shell 的 PATH 而不是 GUI stub
- `tests/agent_parity.rs`：`codex` 的同一條鏈路，加上兩支 CLI 的每個 flag
  與每個 flag 的值都對它們自己的 `--help` 檢查一遍。完全不需要憑證；CLI 沒
  安裝時會大聲跳過
- `tests/hooks.rs`：完整鏈路 PTY → 真的 `claude` → plugin hook → curl →
  HTTP listener，且 session id 正確對應。不需要花錢的 API call
- `ui/tests/fixtures/claude-tui.json`：從 PTY 擷取的真實 Claude Code TUI 輸出，
  **刻意從一個多位元組字元中間切成兩塊**。有一個對照測試證明這份 fixture 用逐
  chunk 解碼確實會壞掉，所以主測試不會因為錯的理由而通過
- `tests/prompt_injection.rs`：在一個真的、沒被信任過的新 worktree 裡跑真的
  `claude`，數 `UserPromptSubmit` hook 觸發幾次。多行 prompt 必須是**一則**
  訊息，不是一行一則
- `tests/worktree.rs`：對真的 git，兩個 attempt 看不到彼此的檔案、各自的
  base_sha 不會互相飄移、worktree 收得回來、分支留著
- `tests/attempts.rs`：整條 core 流程，agent 用替身而不是真的模型。驗的是
  Marol 做了什麼（開哪個 worktree、命令列長什麼樣、記了什麼、還了什麼），
  這些都不需要模型回答。替身的 log 是 NUL 分隔的，因為用一行一個參數會分不出
  「一個含換行的參數」和「好幾個參數」，而那正是這裡要驗的東西
- `tests/attempts.rs` 的時間軸段：完整鏈路 hook listener → router → channel →
  writer thread → SQLite。同時釘住不該記的不要記：連續三次 `running` 只留工具
  呼叫，不留三行狀態
- `pty.rs` 的 tmux 段：在完全沒有 client 的情況下，session 仍在、agent 程序
  仍在跑。那就是整個功能，所以直接驗它，而不是從「重新接得上」去推論
- `store.rs` 的 migration 段：三條升級路徑各一個測試，包括**沒有版本號但已經
  有 `completed` 的舊 DB**（這條沒處理好會讓每個既有安裝都開不起來）、更舊的
  沒有 `completed`、以及從上一版正常升級且資料不掉
- `ui/tests/queue.spec.ts`：排隊的卡片會自己起跑（沒有人按任何東西），以及會
  弄丟工作的合併必須被擋下來並把原因講完
- `ui/tests/board.spec.ts`：兩軸真的成立。卡片留在原欄位不動，燈號自己從
  「等你確認資料夾」→「執行中」→「⚠ 等你授權」變化；點下去之後
  **`document.activeElement` 真的落在那個 pane 裡面**，不只是 pane 有 focused
  class。拖曳測試把四個 drag 事件在同一個 tick 內送完，比真實拖曳更嚴格，
  這樣「靠 React state 剛好 render 完才會過」的實作會當場失敗
- `ui/tests/layout.spec.ts`：在七種視窗尺寸下，沒有任何東西被畫到別的東西
  上面、頁面不橫向捲動、而且沒有任何兩張卡片高度相差超過一個像素。第一條檢查
  直接說出病因（網格被壓得比內容矮）而不是只驗症狀，因為症狀要卡片夠高才看得
  見，病因永遠看得見
- `ui/tests/i18n.spec.ts`：沒選過語言時跟隨系統、選過就以選的為準、切換當下就
  重繪且重開仍在，以及語言確實有送到後端讓原生通知跟著換
- `ui/tests/journeys/`：五條真實的使用動線從頭走到尾，不是逐個畫面戳一下。冷
  啟動的第一次使用一路走到合併；零滑鼠的 triage 日，那份 spec 裡一個 `.click`
  都沒有，鍵盤的承諾是用結構保證的；重啟復原；reduced motion 下的無障礙契約；
  以及同一條線用繁體中文再走一遍。旁邊六張視覺基準釘住關鍵畫面

兩個會去驅動真的 `claude` 的測試（`tests/hooks.rs` 與
`tests/prompt_injection.rs`）在沒有登入好的 CLI 時會自己跳過。**只檢查 `PATH`
上有沒有是不夠的**：沒登入過的 CLI 會停在歡迎畫面、永遠不會開始一個 session，
於是測試會把整個 timeout 燒完，只證明了這台機器沒登入。所以改成去讀 Claude
Code 自己的 `~/.claude.json` 裡的 `hasCompletedOnboarding`。那個 key 哪天換了
位置的話，這些測試會變成跳過而不是錯誤地通過，而且會在 stderr 說明原因。要
強制跑就 `MAROL_TEST_ASSUME_CLAUDE=1`。

### README 的圖與影片

上面的截圖與短片都是真的 React 樹、真的 stylesheet，以及 xterm 渲染真的擷取
下來的 Claude Code TUI。只有後端是每個測試都信任的那份 mock，所以資料是擺
出來的，像素不是。

```bash
SHOTS=1 npm --prefix ui run test:e2e -- shots            # docs/media/*.png
CLIP_DIR=.rec npm --prefix ui run test:e2e -- clips      # 錄影
node ui/scripts/readme-clips.mjs                         # docs/media/clips/**/*.gif
```

每支短片有自己的調色盤。一支大影片共用一張全域調色盤，正是舊錄影顏色跑掉的
原因：256 個位子要同時吃下終端機的語法上色、四種狀態色，以及 diff 的紅與綠，
於是全部往當下最強勢的那一群偏移。一支短片只講一個功能，它的調色盤也就只要
裝得下一個功能的顏色。

---

## 發佈

三個平台的安裝檔由 GitHub Actions 產生（`.github/workflows/release.yml`）。

發版是一個按鈕加一個決定：**Actions → Release → Run workflow → 選 `bump`**，
`patch` 修 bug、`minor` 加功能、`major` 破壞相容。run 會自己算下一版、寫進
`tauri.conf.json`、`Cargo.toml`、`Cargo.lock`、`package.json` 四個檔案、commit
回 `main`，再從那個 commit 建四平台並發佈。沒有人手動維護版本號，所以它每一版
**必然**會動。

接著：建 draft release、四個平台平行 build、**全綠才把 release 轉正**。有平台
掛掉就停在 draft，不會出半套。版本號 guard 仍守著手動路徑：推 tag（或 dispatch
填明確的 `tag`）時，tag 跟 `tauri.conf.json` 不一致就直接失敗，免得 `v0.2.0`
的 release 裡掛著一堆 `Marol_0.1.0_*`。明確的 `tag` 也是失敗重跑的路徑，
因為 bump commit 已經落地但發佈失敗時，要用已經燒掉的那個 tag 重發，不是再
bump 一次。

### nightly build

每次 push 到 `main` 都會跑同一套四平台 build，並發佈到一個 tag 為 `nightly`
的滾動 prerelease，蓋掉上一份。所以 `main` 的最新版本永遠一個連結就拿得到，
不用等誰去發版：

    https://github.com/KCL1104/marol/releases/tag/nightly

它是 prerelease，而且**永遠不會被標成 latest**，不會擠掉正式版在 repo 首頁與
release API 上的位置。有平台失敗的話，那份 draft 會被丟掉、上一份 nightly
留著，不會出半套。build 途中又有新 commit 進來會直接取代它（只有最新的產物
有意義），但 tag 的 build 永遠不會被取消。

這也是為什麼 `ci.yml` 不再打包：它以前每次 push 到 main 都建三個平台，然後
整包丟掉。

沒有任何發版路徑會用 git 推 tag。tag 一律由 GitHub 在 release 發佈時建在
build 的那個 commit 上，跟 nightly 的 tag 同一套機制。兩個輸入都留空則只
build，產物掛在該次 run 的 artifacts 底下，不碰任何 release。其實每次 run
都會掛，所以正式版與 nightly 的 build 也都能直接從 run 裡下載。

| 平台 | runner | 產物 |
| --- | --- | --- |
| Linux x86_64 | `ubuntu-22.04` | `.deb`、`.rpm`、`.AppImage` |
| macOS Apple Silicon | `macos-15` | `.dmg`、`.app` |
| macOS Intel | `macos-15-intel` | `.dmg`、`.app` |
| Windows x86_64 | `windows-latest` | `.msi`、NSIS `.exe` |

Linux 建在 22.04 而不是 24.04：glibc 與 WebKit 只往前相容，24.04 建出來的東西
在 22.04 上跑不起來。`macos-15-intel` 是 Actions 最後一版 x86_64 的 macOS
image，2027 年 8 月退役，到那時候 Intel 那一列就得拿掉。

`.deb` 與 `.rpm` 的相依只有一半會自己長出來：bundler 會去讀執行檔實際連到的
so，把 `libwebkit2gtk-4.1-0`、`libgtk-3-0` 補進去。**但 `git` 不會**，它是用
`Command::new("git")` 在執行期叫的，不是連進去的函式庫，掃不到。所以那一條
寫在 `tauri.conf.json` 的 `bundle.linux.deb.depends` 裡，漏了的話裝得起來、
開下去 worktree 就爛掉。`gh` 放在 `recommends`，只有開 PR 那條路徑會用到。

### 沒有簽章

repo 裡沒有任何簽章金鑰，所以三個平台的產物都是未簽章的。使用者第一次開會被
系統擋：

- **macOS。** Gatekeeper 會說「已損毀，無法打開」。不是真的壞掉，是 quarantine
  屬性：

  ```bash
  xattr -dr com.apple.quarantine /Applications/Marol.app
  ```

- **Windows。** SmartScreen 藍色視窗，「更多資訊」→「仍要執行」
- **Linux。** 不擋

要正式簽章的話，把 `APPLE_CERTIFICATE`、`APPLE_CERTIFICATE_PASSWORD`、
`APPLE_SIGNING_IDENTITY`、`APPLE_ID`、`APPLE_PASSWORD`、`APPLE_TEAM_ID` 加進
repo secrets，然後在 `release.yml` 的 build step 上把它們接成 `env`。那裡有
註解標了位置。

**刻意不預先接好。** bundler 判斷要不要簽看的是 `APPLE_CERTIFICATE`
**存不存在**，空字串也算存在，它不會去檢查有沒有值。所以在一個沒有這些
secrets 的 repo 裡去引用它們，等於把變數設成 `""`，兩個 macOS job 就會死在
`failed codesign application: failed to import keychain certificate`。要加就
跟真正的 secrets 同一次加，不要提前。

**有一個安慰，而且是真的：更新不會經過 Gatekeeper。** quarantine 屬性是由
**下載**檔案的那個程式掛上去的，而就地更新是 app 自己抓的，不是瀏覽器。所以
上面那行 `xattr` 是第一次安裝才付的成本，付一次；之後每一個版本都不用 ——
即使什麼都還沒簽章。

### 更新簽章

更新簽章是**另一把金鑰**，工作也不一樣：它簽的是 manifest 和產物，讓正在跑的
Marol 能證明剛下載的位元組來自這個 repo。Apple 的金鑰是向作業系統擔保這個
app；這一把是向 app 擔保這次更新。

這裡同樣沒有這把金鑰，所以釋出的 build 帶著空的 `pubkey`，並且會在更新按鈕
原本的位置說「這個 build 沒有帶更新用的金鑰」。要啟用：

```bash
npm run tauri signer generate -- -w ~/.marol-updater.key
```

它會印出公鑰並寫出一把私鑰。然後，**在同一次變更裡**：

1. 把公鑰貼進 `src-tauri/tauri.conf.json` 的 `plugins.updater.pubkey`。
2. 把 `TAURI_SIGNING_PRIVATE_KEY`（私鑰檔的內容）和
   `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` 加進 repo secrets。

剩下的 `release.yml` 會處理：那一次 run 會把 `createUpdaterArtifacts` 打開、
簽章，並在安裝檔旁邊上傳一份 `latest.json`，那就是 app 的 endpoint 會去讀的
檔案。沒有那個 secret 的 run 則整套都不開，產出跟以前一模一樣的 release ——
這正是兩半必須一起落地的原因，也是為什麼 workflow 在**只有 secret、沒有
`pubkey`** 時會直接大聲失敗，而不是送出一個無法驗證自己更新的 app。

產金鑰之前值得先知道的兩件事：

- **私鑰弄丟就再也無法對既有安裝推更新。** 已經在外面的每一份，只信任編進它
  自己裡面的那半公鑰。備份到這台筆電以外的地方。
- **金鑰存在之前做的 build 永遠無法自己更新**，同樣的道理 —— 它們裡面沒有
  公鑰可以拿來驗簽。裝了那種版本的人，得手動再裝一次。

### icon

`src-tauri/icons/` 底下的 `.ico`、`.icns` 與各尺寸 PNG 都是 commit 進來的，
不是 CI 現產。Windows 要 `.ico`、macOS 要 `.icns`，少一個那個平台就打不出
安裝檔。要換圖時：

```bash
npm run tauri -- icon path/to/new-icon.png
```

它的輸出目錄預設就是 `src-tauri/icons/`，而且**會連來源的 `icon.png` 一起
覆寫**。想留著原圖就先 `-o` 到別的地方，再把需要的檔案搬回來。

---

## CI

`.github/workflows/ci.yml`。push 到 main 與所有 PR 都會跑：Rust `cargo test`、
前端 typecheck 加 build 加 Playwright、sidecar typecheck 加 build。**只管正確
性**。打包是 release.yml 的事，而且 push 到 main 那一輪會產出真的能下載的
安裝檔，而不是建完就刪掉。

`cargo fmt` 與 `clippy` **不擋 CI**，只把結果印出來。現在這棵樹還不是
rustfmt-clean，把整棵樹重排是另一件事，不該跟接 CI 綁在一起。

`npm run smoke` 沒有進 CI：它會真的開一個 Claude Code session，需要憑證。

`.github/workflows/claude-detect.yml` 守著其餘 CI 守不住的那一條承諾：app 在
真的機器上找得到真的 agent CLI。四條腿，Linux、macOS、原生 Windows、WSL 裡
的 Ubuntu，各自在真的 runner 上裝真的 CLI，然後驅動 app **自己**的解析路徑
（login-shell 探測、各平台的 PATH 走法、WSL 那道門）直到找到執行檔、並從
`--version` 拿到回答。WSL 那條腿連 Codex 一起裝，因為受測的是那道門，而一個
世界只在它真的搆得到的 agent 上可用。每次推上 `main`、碰到 `src-tauri` 的
push 都跑，每週一也跑，因為上游安裝器改了形狀不需要這裡有任何 commit；樹是
綠的而週一紅了，指的就是他們。

`.github/workflows/agent-parity.yml` 守著另一條：這個 app **遞給**那些 CLI
的東西，它們是不是還收。`src-tauri/src/agent.rs` 是一張別人家慣例的表，這種
表會安靜地爛掉 —— 改名的 flag 是一個還沒畫出終端機就結束的 session，不再被
認得的設定 key 是一張永遠不顯示狀態的卡片（Codex 對解析不了的 `-c` 值是當成
字串留著，不是拒絕，所以什麼都不會失敗）。所以這張表拿去對真的 CLI 量，
Linux、macOS、Windows 三個平台都跑：

- app 送得出的每一個帶橫線的 token，都要出現在那支 CLI 自己的 `--help` 裡 ——
  它配的每一個值也要，因為 `--sandbox` 活著而 `workspace-write` 改了名，
  失敗得跟 flag 不見一樣徹底
- `codex resume` 還是子命令、`--continue` 還是選項，因為這兩者放在命令列的
  兩端
- `codex doctor` 要把這個 app 送的那組 `-c` 參數回報成「設定載入了」——
  而故意寫壞的那一個要回報成「拒絕了」，否則前半句什麼也沒證明
- 一支真的 `codex` 帶著那些參數啟動，要抵達 app 真的 hook listener，session
  id 由 shell 展開過、payload 在 request body 裡

這些全都不需要憑證：`codex exec` 在第一個請求送出去之前就會觸發
`SessionStart` 與 `UserPromptSubmit`，請求之後才因為沒有認證而失敗，離被量的
那一段很遠。碰到後端的 PR 會跑，每週二也跑。

---

## 語言

介面有英文與繁體中文兩種。

開啟時跟隨系統語言（任何 `zh*` 的 locale 給中文，其餘給英文），設定裡有切換器。
在那裡選過之後，選擇一律蓋過系統設定，而且跨重啟保留。

決定權在 webview，並透過 `set_locale` 往下推給 Rust，所以那少數幾個由 OS 而
不是 webview 繪製的字串，也就是原生通知的標題與內文，會跟著一起換。與其讓兩套
偵測規則各自判斷、然後可能不一致，不如只留一套、另一邊照著做。

介面字串在 `ui/src/i18n/messages.ts`。**英文是真相來源**：它的 key 定義出
`MessageKey` 型別，中文那份被定成對這個型別的全對映，所以只加一邊、忘了另一邊
會直接 typecheck 失敗，而不是靜靜地在畫面上印出一個 raw key。Rust 自己要講的
那幾句在 `src-tauri/src/i18n.rs`。

介面只說某個控制項做什麼。它不會回頭跟你解釋 git、shell 或 CLI：錯誤訊息說出
發生了什麼就停下來，理由留在這裡和初次使用導覽裡，那是刻意只讀一次的地方，
而不是每次出錯都再講一遍。

程式碼註解**刻意留中文**。那是寫給維護的人看的，不是給使用者看的，而且裡面
裝的理由是這個 repo 最值錢的東西。翻它跟「讓產品雙語」是兩件不同的事。

---

## 狀態偵測

多開 session 時你唯一真正需要的資訊是「哪一個在等我」。取得方式是請 agent
自己回報，不是去解析畫面，因為解析 ANSI 會在 TUI 改版時無聲壞掉。

App 啟動時做兩件事：在 loopback 開一個小 HTTP listener，以及把一份 plugin
寫到資料目錄。每個 session 都被注入 `MAROL_SESSION_ID`，並用它那支
CLI 自己提供的方式指向那個 listener —— Claude Code 用 `--plugin-dir` 載入
plugin，Codex 收 `-c hooks.*` 覆寫，那是只屬於這一次啟動的設定、不碰磁碟上
任何檔案。兩種都不寫進你自己的設定檔，因為一個會去改
`~/.claude/settings.json` 或 `~/.codex/config.toml` 的 app，就是一個能悄悄
關掉你自己寫的 hooks 的 app。

這份 plugin 是 hooks，加一個 skill。hooks 只跑在 harness 上，不花模型任何
context；那個 skill 是 session 用來幫自己取名的，也是這個 app 有史以來唯一
放進 agent context 的東西——在 Claude Code 2.1.229 上用
`claude --plugin-dir … plugin details marol-status` 量到每個 session 約 90
tokens。寫在這裡是因為這種形狀的主張應該可以被查證，而不是被宣稱。

| Hook 事件 | 回報狀態 | |
|---|---|---|
| `SessionStart` / `UserPromptSubmit` / `PreToolUse` | 執行中 | 兩支都有 |
| `PermissionRequest`、`Notification`(permission_prompt) | **等你授權** | 兩支都有 |
| `Notification`(idle_prompt) | **等你回覆** | 只有 Claude Code |
| `Stop` | 待命 | 兩支都有 |
| `SessionEnd` | 結束 | 兩支都有 |

Codex 沒有閒置提示事件，所以它從不回報「等你回覆」。沒有任何東西回報得出來
的狀態，這張桌子不會自己發明；Codex 回合結束就是「待命」，那本來就是
「該你了」。

**一張永遠不會回報的卡片會自己說出來。** 這個免責標籤原本對任何慣例已被實測
的 CLI 一律不顯示，理由是它本來就該回報，而一個在第一個 hook 到達時就自己收
回去的標籤是閃爍。但「這張桌子知道怎麼接這支 CLI」不等於「它真的接上了」：
一個比自己 hooks engine 還舊的 Codex 跑得好好的、一句話都不說，而它的卡片跟
一張安靜工作中的卡片長得一模一樣。現在記在啟動當下的是「接線到底有沒有發生」
—— 而且是每個 session 各自記，因為答案是每個世界各自的，distro 裡那支可能夠新
而本機那支不夠。沒接上的 session 永遠不會回報，所以標籤依然不可能閃爍。

只有「等你授權」與「等你回覆」會發通知與計入徽章。那是 agent 真的被擋住、
沒有你就無法繼續的兩種狀態。

三個實作上的地雷（都是實測出來的，文件沒寫）：

1. **不能用 `--settings` 塞 hooks。** 它會覆蓋同名 key，等於把你自己的 hooks
   整個關掉。plugin hooks 才是附加的。
2. **`"shell": "sh"` 會讓 hook 靜默不觸發。** 沒有錯誤、沒有回報。`"bash"`
   可以，不指定也可以。有回歸測試釘住這點。
3. **hook 一定要 exit 0。** 退出碼 2 會**擋下**它所在的那個工具呼叫，所以每
   一行都以 `|| true` 結尾（Codex 那邊是 `|| exit 0`，因為這句話在 `sh` 與
   `cmd.exe` 裡意思一樣，而 `cmd.exe` 根本沒有 `true` 這個命令）。app 掛掉
   絕不能連帶卡死 agent。

再四個，量的是 Codex 0.147，外加讀它的原始碼：

4. **Codex 沒有 `http` 這種 hook type**，所以每個事件都要付一次 `curl` ——
   而它預設的 hook timeout 是十分鐘。一個能把工具呼叫卡住十分鐘的狀態回報，
   比沒有狀態更糟，所以這個 app 設定的每個 hook 都帶很短的 timeout，裡面的
   `curl` 還更早放棄。
5. **Codex 的 hook 沒被信任過就不會跑**，而信任是記在 hook 自己的雜湊上。
   所以這份定義每個 session 都一模一樣 —— session id 走 `$MAROL_SESSION_ID`
   而不是寫死 —— 一次 `/hooks` 就管一台機器一輩子。它現在擋住的不只是狀態：
   `SessionStart` 正是告訴 Codex session 怎麼跟其他 session 傳訊的那個 hook，
   所以在 `/hooks` 被回答之前，它既不回報、也不知道有這條通道。
6. **不用 `$` 拼變數的 shell 會讓那個 id 原樣留著。** 每份 hook payload 都帶
   工作目錄，而一個 attempt 的 worktree 只屬於一個 session，所以 id 沒活著
   抵達的回報改用目錄安放。同一個目錄下有兩個活著的 session 就拒絕，不猜。
7. **`SessionStart` hook 可以遞給 Codex 一段 context，不只是一則回報。**
   在 stdout 回傳 `hookSpecificOutput.additionalContext`，Codex 會把它記成
   對話上的一則 developer message —— 那是 Codex 唯一提供的、每次啟動都能教
   session 一件事的門。這一條是讀 Codex 原始碼來的，不是從 binary 量出來的，
   而且在用到它的地方就這麼寫著：萬一它變了，Codex session 只是回到不知道有
   這條通道，其他什麼都不會壞。

（另外三個關於 worktree 與首則 prompt 的實測結果，見下面「任務與 attempt」。）

---

## 任務與 attempt

`Task 1 ─ N Attempt 1 ─ 1 Session`。Attempt 是「用某個 agent 試做這張卡的一次
嘗試」，帶自己的 worktree 與分支；換 agent 重跑就是開新的 attempt。

一張卡指名**一個或多個 repo**，attempt 會在每一個裡面各開一棵 worktree，全部
用同一個分支名。只有一個 repo 時 —— 絕大多數的卡 —— checkout 就放在 attempt
自己的路徑上，和過去完全一樣。多個時，每個 repo 在那個路徑底下各占一個以
repo 為名的資料夾，attempt 的路徑就成了 session 起跑的工作區。後面每一件事都
涵蓋全部：diff 是一份 diff，路徑相對於那個工作區算（`web/api.ts`、
`api/routes.py`），所以審查留言指的路徑，agent 站在原地就打得開；合併是好幾次
合併，而且**每一個的拒絕條件都在任何一次動手之前先問完**；park 把每一棵
worktree 都還回去，resume 再把它們全部長回來。

安全論證沒有變，而且那是設計目標，不是碰巧。agent 碰得到的每個 repo 仍然是這
次 attempt 自己分支上的 worktree，沒有一個是你本人的 checkout。attempt 能花掉
的仍然只有自己的分支 —— 只是現在有好幾條。建卡時有兩條拒絕守住這條線：這些
repo 必須**在同一個世界**（worktree 共用一個資料夾，而資料夾跨不過通往 WSL
distro 或 SSH host 的那道門），以及**同一個 repo 不能出現兩次**（那是同一條分支
的兩棵 worktree，git 本來就拒絕，而且後面沒有任何東西分得出來）。

狀態分兩軸，而且**軸二絕不自動驅動軸一**：

| 軸 | 內容 | 誰決定 |
|---|---|---|
| 一・任務生命週期 | `backlog → running → review → done` / `abandoned` | 只有人，用拖的 |
| 二・session 即時狀態 | 執行中 / ⚠等你授權 / ⚠等你回覆 / ⚠等你確認資料夾 / 待命 / 執行中，尚未回報 / 結束 | hook 回報 |

沿用 `store.rs` 既有的 `completed` 立場：`Stop` 只代表這一輪結束，不代表事情
做完了，所以沒有任何 hook 能搬動卡片。

worktree 放在 `~/.marol/worktrees/<repo>-<hash>/<slug>-<n>/`，**不放在
repo 旁邊** —— 跨多個 repo 的卡，最後那層目錄就是工作區，底下每個 repo 各
一棵 checkout。repo 的上層目錄很常自己也是一個 repo（傘狀 workspace），worktree
放進去就變成巢狀 repo，所有往上找 `.git` 的工具都會開始給出不一樣的答案。也
不放在 application support 底下：這是人會想 `cd` 進去、用編輯器打開、在裡面
跑 build 的工作目錄，「打得出來的路徑」比「整齊」值錢。

又三個實測出來、文件沒寫的事實（`tests/prompt_injection.rs` 釘住）：

4. **位置參數傳 prompt 不會退化成 print 模式**，`-p` 才會。多行字串經 argv
   傳進去是**一則**訊息，因為 argv 裡的換行是文字，不是 Enter。
5. **新 worktree 一定會撞信任對話框，而且在答完之前什麼都不會跑，
   `SessionStart` 也不會。** 所以沒有任何 hook 能回報這個狀態；core 直接標成
   `AwaitingTrust`，它有資格這樣做，因為那個目錄是它前一刻自己建的。少了這個，
   徽章就會漏掉每個 attempt 的第一個狀態。prompt 本身能活過對話框，答完就
   送出。
6. **`$SHELL -ilc` 會繼承 Marol 自己的環境。** 從 Finder 啟動時那是乾淨
   的，從 Claude Code session 裡的終端機啟動就不是，因為
   `CLAUDE_CODE_CHILD_SESSION` 會關掉 transcript 儲存，於是 `--continue` 沒有
   東西可以接，重開 attempt 會無聲地從頭開始。`shell_env` 會把這類 session
   marker 拿掉，但**只拿掉明確列出的那幾個**：`CLAUDE_CODE_*` 底下也住著
   `CLAUDE_CODE_USE_BEDROCK` 這種真的使用者設定，用前綴一律砍會把別人的環境
   弄壞。

首則 prompt 只注入 agent 自己發現不了的事：這是為這張卡開的地、分支是哪個、
從哪個 base 開出、commit 在這個分支上。CLAUDE.md、skills、MCP 都會原生載入，
不重複塞。模板在 `<data_dir>/prompt-template.md`，可以改，升級不會蓋掉。
開 attempt 的對話框顯示完整 prompt 且可編輯，送出什麼就記什麼。

`{repos}` 就是說「是什麼樣的地」的那個 placeholder：一棵 worktree 和它的分支，
或者 —— 跨多個 repo 的卡 —— 這是一個工作區，底下哪個資料夾是哪個 repo。因為
模板永遠不會被覆蓋，**今天硬碟上每一份模板都是在「一張卡一個 repo」的世界寫
的，沒有一份提到 `{repos}`**。所以它沿用 `{prompt}` 早就有的那條規則：這張卡
真的跨了多個 repo、而算出來的文字從頭到尾沒說，那段就自動補上去。一個站在工
作區裡、卻被告知自己在一棵 worktree 裡的 agent，會去它醒來的目錄找檔案，然後
找到一堆資料夾。只有一個 repo 的卡什麼都不加 —— 那份模板自己的句子，對它的處
境已經句句為真。

沒實測過的 agent 不自動送 prompt：那些 CLI 的參數慣例不知道，而在某個 CLI
裡代表「這是你的 prompt」的參數，在另一個裡可能代表「印出來然後結束」。
猜錯比不猜更糟，所以 UI 顯示組好的 prompt 讓人一鍵複製。

### 兩支實測過的 agent

Claude Code 與 Codex 是這張桌子知道其慣例的兩支 CLI，它們拿到的東西一樣：
首則 prompt 走命令列、review 批次從 session 自己的輸入送回去、權限模式、
接回那個目錄裡既有對話的 resume、來自 hooks 的狀態與活動、從 transcript
讀出來的 token 帳。這些慣例全部住在同一張表 `src-tauri/src/agent.rs`，
所以第三支 agent 是加一列，不是把整個核心重新稽核一遍。

它們不是彼此的翻譯，這裡也不假裝是：

| | Claude Code | Codex |
|---|---|---|
| 首則 prompt | positional | positional |
| resume | `--continue`（選項） | `resume --last`（子命令） |
| 自動接受編輯 | `--permission-mode acceptEdits` | `--sandbox workspace-write --ask-for-approval on-request` |
| 全自動 | `--dangerously-skip-permissions` | `--dangerously-bypass-approvals-and-sandbox` |
| hooks | plugin，走 `--plugin-dir` | 設定，走 `-c hooks.*` |
| 閒置提示 | 會回報 | 沒有這個事件 —— 回合結束就是「該你了」 |
| session 名字 | `--name`，CLI 自己也以此互傳訊息 | 沒有 |
| 卡片之間傳訊 | 走 Marol 自己的通道 | 同一條通道 |
| token 記帳 | 一則訊息一列 | 累計總和 |
| 保持最新 | `claude update` | `codex update` |

**讓它們保持最新是「請它們去做」，不是自己動手。** 兩支 CLI 都帶著自己的
updater，會判斷**這一份**是怎麼裝的 —— npm 全域、原生安裝器、Homebrew
cask、apt 套件 —— 然後跑那種方式的升級，所以 Marol 只負責問，不猜。猜正是
會出事的那一半：`npm install -g` 蓋在原生安裝上並不會取代它，而是多裝一份，
然後把「哪一個 `claude` 會跑」交給 `PATH` 先命中的那個目錄決定。

它同時也是那個「檢查」。兩支都沒有只看不動的模式 —— Codex 那個連旗標都不
收 —— 所以「有沒有新版」跟「拿到它」是同一個命令，已經是最新的那支會自己
這樣回答。

這件事每個世界跑一次，在 Marol 第一次連到那個世界的時候；對你在用的世界來
說就是開啟的當下。之所以按世界，是因為 CLI 本來就是按世界的：一個 WSL
distro 有它自己的 `claude`、自己的版本，而更新 Windows 那邊那支，會讓真正
在跑的那支原封不動。它不在啟動的關鍵路徑上，也沒有任何東西等它 —— 兩支
CLI 都是把新版裝在執行中的那份**旁邊**，交給你下一個開的 session，不會動到
已經開著的；更新失敗就是 CLI 留在 Marol 原本探到、也是拿來開關旗標的那個
版本。之後版本會重新探一次，因為之後的每一次啟動都必須由「真正會拿到的那個
binary」決定，而不是由升級前一刻量到的那個。

某個世界裡沒裝的 CLI 就放著。在別人的機器上**裝**一個 agent，跟更新一個他
自己選擇要有的，是兩件事。

**設定 → 更新裡有開關，也有報告。** 預設開，因為被你正在講話的那個東西叫去
更新，本來就是整件事最煩的地方。之所以留下「關」，是因為這個 repo 是**量測**
這些 CLI 的 —— `agent-parity.yml` 整個 workflow 就是為了抓它們其中一支改掉
旗標名字 —— 而一次沒人看著的升級，正是有人踩到那個變動的路徑。報告會說每個
世界改了什麼、從哪個版本到哪個版本，被拒絕的也會指名，不會被摺進沉默裡。
它是靠比對升級前後探到的版本判定的，不是讀 CLI 自己的成功訊息：那是文字，
而文字改過。

有一件事它刻意不做：Homebrew、WinGet 和 Linux 的套件管理器都不會自動更新，
而 Claude Code 只有在 `CLAUDE_CODE_PACKAGE_MANAGER_AUTO_UPDATE=1` 有設的時候
才會替你去跑它們的升級。Marol 不設這個。`brew upgrade` 是系統套件層級的操作，
伸手進別人的套件管理器，比「幫我把 agent 保持最新」所要求的走得更遠 —— 如果
你是那種安裝方式，自己設。

兩種接法都不寫進你自己的設定檔。一個會把自己塞進
`~/.claude/settings.json` 或 `~/.codex/config.toml` 的 app，就是一個能悄悄
關掉你自己寫的 hooks 的 app。

**Codex 會請你信任它的 hooks，一次。** Codex 不跑沒給它看過的 hook，而且把
信任記在 hook 自己的雜湊上。所以第一個 Codex session 會在它自己的終端機裡、
用它自己的話說 hooks 需要審核；`/hooks` 回答它一次，之後每個 Codex session
都會回報狀態，因為這張桌子每次送的都是同一份 hook 定義。session id 走
`$MAROL_SESSION_ID` 而不是直接寫死，正是為了這個。Marol 不送
`--dangerously-bypass-hook-trust` —— 那會連 repository 自己帶的 hooks 一起
放行。

---

## 會互相說話的 session

同一張看板上的卡片常常在動同一份程式碼，而其中一張學到的事，往往正是另一張
需要的。Claude Code 有一個功能就是做這件事，Marol 也把它打開了 —— `--name`
讓每個 session 帶著它自己那一列的名字，訊息才有地方去。但它回答的問題比這張
桌子問的小。那是 Claude Code 的東西，所以 Codex session 既不能用它、也不能被
它定址；而且它是 per machine 的，一個 `/tmp` 底下的 socket 加一份
`~/.claude` 裡的註冊表，而一張桌子動不動就橫跨 WSL distro 和 SSH host，
兩邊的檔案系統一樣都不共用。

所以有第二條通道，是這張桌子自己的，兩支實測過的 CLI 都可以站在任一端。每個
接上了的 session 都拿到兩個屬於自己的位址：

```bash
curl -sS --max-time 3 "$MAROL_PEERS_URL"       # id<TAB>名字<TAB>狀態，一行一個
curl -sS --max-time 3 -X POST "$MAROL_SEND_URL" \
  -H "X-Marol-To: <id>" --data-binary "auth.py 我在動，先別碰"
```

出去走的是狀態 listener，它本來就跨得過 WSL 掛載和 SSH 隧道；進來走的是遞送
人自己那則追加訊息的同一次貼上。兩半都不是新的；缺的是「怎麼問誰在這裡」、
「怎麼說這則要給誰」，以及一個值得信任的身分。

**用 id 定址，不用名字。** 名字是人寫的句子，可能有引號、空白、換行；id 是
uuid。這一個選擇就是為什麼整條通道不需要任何 escaping、percent-encoding 或
JSON —— 兩個變數照原樣用，id 走 header，訊息就是 body。

**每個 session 一個 token。** 光靠 `sid` 是一個兄弟 session 從自己環境裡就讀
得到的 uuid，而這條通道是把文字放進另一個 agent，不是回報自己。每個接上的
session 拿到一個為那次啟動鑄的 token —— 不落地，也不放在送進視窗的 session
列上，那等於把 token 放進網頁裡。`$MAROL_NAME_URL` 刻意不帶 token：偽造一次
改名，最壞也只是換掉一個列名。

**它抵達時戴著標記。** 人打的追加訊息帶著人的權限；從另一個 agent 中繼來的
訊息走同一個鍵盤進來，就不能帶。所以它被包在一個框裡，說明它來自另一個
session、是哪一個、以及它不代表那個人 —— 最後那句是承重的，因為沒有它，一個
peer 就能叫一個在寬鬆模式下無人值守的 agent 做任何使用者能做的事。另一個
agent 不能替你批准任何東西，而這句話在 agent 讀到內容之前就先說了。

**遞送等回合結束。** 訊息是排隊而不是直接打進去，因為對方可能正在回合中間，
而落在回合中間的貼上會去操縱它而不是回答它；不在回合中間的對象則立刻排空。
佇列有上限，而且滿了是一個答案不是無聲丟棄 —— 寄件者是一個能對「滿了」做出
反應的 agent。好幾則訊息會變成一個回合裡的好幾段，絕不會變成好幾個回合。

**時間線說是誰講的。** 中繼來的訊息是自己一種列，指名寄件者，而且不帶還原
錨點 —— 還原屬於人開始的回合，這不是其中之一。把它記成 prompt，等於對事後
讀紀錄的人說了那個框正在阻止對 agent 說的同一個謊。

**這條鏈有天花板，而下來的路是你。** 兩個 agent 互相回答，是佇列看不見的失控：
兩邊都不會同時握著超過一則訊息，所以一對 session 可以一路對答到 app 關掉，什麼
上限都填不滿。失控的是那條鏈，而鏈上每一環都是一整個回合，錢是沒在看的人付的。
所以每個 session 身上都帶著「它最後被告知的事，離人上一次說話有多遠」—— 人是
零，每中繼一次加一 —— 超過八手，這張桌子就拒絕再送，並且告訴寄件者去問鍵盤前的
那個人。往任何一邊的終端機打字都會把計數歸零，因為那正是這個天花板要求的那份
看顧。抽屜從第一手就把深度顯示出來，所以你可以看著三變成五，在有東西被代替你
拒絕之前先介入；而真的被擋下來的時候，卡片會說。

**Codex 是走它自己的 hook 進來的，因為它沒有別條路。** Codex 會把交給 model 的
shell 關進沙箱，而且只放行 `AF_UNIX` 這一個 socket family —— seccomp 過濾器只在
第一個參數是 `AF_UNIX` 時允許 `socket()`，loopback 沒有例外。所以一個打向這張桌子
的 `curl` 會在 `socket()` 裡面就失敗，而 model 看到連線錯誤，就會回報「桌子掛了」
—— 它其實好好的。這不是靠打開沙箱來繞過的事：沙箱本來就是重點。

通得過的那條路，是本來就在運作的那一條。Codex 的 *hook* 是它自己在跑的，在它自己的
行程裡，在那個沙箱外面 —— 這正是為什麼一個 `curl` 出不去的 session，狀態卻照常回報
—— 而 `PreToolUse` hook 會拿到 model 正要執行的那行指令。所以 model 把它想做的事
寫成一個 shell 的 no-op：

```bash
: marol-peers
: marol-send <對方的 id> <你的訊息>
```

沒有任何東西會執行它們；開頭那個冒號是 shell 的 no-op。Marol 從它本來每個指令都會
收到的那個 hook 裡把這行讀出來，把事情做掉，然後用那個 hook 自己的
`additionalContext` 回答，Codex 會把它記在對話上。進去一條路、出來一條路，兩條都不是
model 被沒收的那種 socket。

門底下是同一份程式：同一個佇列、同一個信封、同一條中繼天花板。不同的只有這個
「請求是怎麼進來的」，以及隨之而來的驗證方式 —— 這條路上沒有 per-session token。
它不需要：呼叫者是 Codex 自己的 hook 行程，它是用 hook URL 裡那個秘密連上監聽器的，
而任何能偽造這個的東西，本來就已經能偽造狀態了。

**每支 CLI 走自己那扇門學會它。** Claude Code 從 `--plugin-dir` 帶進去的
plugin 裡讀一份 skill。Codex 沒有等價的 per-launch 機制 —— 它的 skill 住在
`~/.codex/skills`，那是人自己的設定，這個 app 不寫進去 —— 所以它由自己的
`SessionStart` hook 告知，那個 hook 可以回傳 `additionalContext`，Codex 會把
它記在對話上。兩支 CLI 的設定檔都沒有被寫進任何東西，而 Codex session 不用被
教任何事就已經能**收**訊。

---

## 架構

```
Tauri 視窗 (React + xterm.js)
      │  invoke: term_write / term_resize
      │  event:  term:output
Rust 核心  ── PTY registry · session 清單 · SQLite
      │  portable-pty（agent 由各自世界的 tmux 扛著，一個 session 一個 socket）
  claude / codex / … × N
```

核心（`src-tauri/src/core.rs`）不依賴 Tauri，只透過 `UiSink` trait 對外，
之後要加 axum websocket 讓瀏覽器或遠端連進來不必重寫。

### 一扇門的代價

Marol 在 WSL distro 裡或 SSH host 上做的每一件事，本來都是它自己的一個
`wsl.exe` 或 `ssh`，而在 Windows 上，process 才是貴的那一部分。這件事在本機
永遠看不出來，因為本機的同一批呼叫是 `std::fs`，成本是微秒級 —— 兩條路差了三
個數量級，在原始碼裡看起來卻一模一樣。

三件事把它關掉，而順序有意義：第一件讓視窗不再凍住，第二和第三件才真的把工作
變少。

- **沒有任何 command 跑在視窗自己的執行緒上。** 同步的
  `#[tauri::command]` 會把整個函式主體跑在主執行緒上，而在 Windows 上，帶著
  invoke 的那個 WebView2 handler 也在那裡觸發。所以一次卡片刷新就是 300 毫秒
  的視窗不重繪、輸入不處理、終端機輸出也送不進 webview。現在那些工作交給一個
  blocking pool —— 刻意不是交給 async runtime，因為每個 agent 回報狀態的那個
  hook listener 就住在上面，餓死**它**會把一張慢的桌子變成一個慢的 agent。
  `term_write` 和 `term_resize` 刻意保持同步：一次按鍵是往這個行程已經握著的
  pipe 寫一次，而 blocking pool 之間沒有順序保證，搬過去可能讓兩個快速按鍵倒
  序抵達。
- **一次讀取付一個 process 給答案，不是給問題。** 看板的 footprint 本來是四次
  git 呼叫加上每個未追蹤檔案一次，而且每張開著的卡每十五秒問一次；規則檔分頁
  本來是六次存在檢查、四次列目錄、再加每個 skill 一次檢查。那些迴圈沒有消失
  —— 對 `/dev/null` 做 `--no-index` 仍然是把新檔案畫成「會建立它的那個 patch」
  的方法 —— 它們搬到門的另一邊，變成一份會把各段印回來的腳本。
- **一個世界握著一個 shell。** 每個世界一個 `sh`，之後每個指令都是往它的 stdin
  寫一行，所以那些讀取一個 process 都不用付。它是最佳化，絕不是依賴：撐不起
  shell 的世界、大到不該塞進 pipe 的指令，或單純只是每個 shell 都忙著，全都退回
  用舊方式生一個 process。框架數位元組而不是用結束標記，因為輸出是 bytes，而任何
  終止符遲早會出現在某個被讀取的檔案裡。

  有一種失敗絕對不能退回，而把它跟其他失敗分開，正是那個三選一的答案的意義。
  指令一旦被寫進 shell，shell 之後安靜下來並不代表它沒跑 —— `git commit` 是寫完
  commit 之後管線才斷的，再生一次就是第二個 commit。所以「送出去之後」的失敗是
  *遺失*，不是*婉拒*：往上拋給你，而不是重試，並且講明它拒絕替你猜的是哪件事。

  安靜五分鐘的 shell 會被放棄並殺掉。這不是延遲預算 —— 這裡從來就沒有過，而緊到
  能當預算的值會把一次慢的 clone 判成遺失的指令。它是「這個 shell 不是慢，是卡住
  了」的那條線，好讓它佔住的位子和卡在它上面的執行緒回得來，而不是整個 app 的
  壽命都沒了。

  婉拒是安靜而且正確的，而這正是它要被算的理由：一個門後沒有 `sh` 的世界，用起來
  跟通道運作良好的世界一模一樣，只是比較慢，而且沒有任何地方會說。診斷分區現在
  每個世界一列 —— 幾個指令沒開 process 就有答案、總共幾個，遺失的另外指名。

本機完全不套用批次也沒有通道，理由有兩個：沒有門要攤提，而且 `sh` 不在 Windows
的 login-shell PATH 上。有一條測試會數一個真的 `wsl://` attempt 過了幾次門，
並把它釘在零 —— 因為用上限會讓一個「每張卡每十五秒多一次」的回歸照樣通過。

### 為什麼要 login-shell 環境解析

從 Finder 或 Dock 啟動的 GUI 程式拿到的是精簡環境：`PATH` 大約只有
`/usr/bin:/bin:/usr/sbin:/sbin`，沒有 nvm/mise/asdf 的 shim、沒有 Homebrew
前綴、沒有你 export 的 API key。把這種環境交給 coding agent，`npx` 型的 MCP
server 起不來，常常連 agent 本身都找不到。

`shell_env.rs` 啟動時跑一次 `$SHELL -ilc 'env -0'`，用你自己 shell 的環境去生
所有 session。設定裡的診斷分區會顯示解析結果，降級時也會明講。

同一套解析在原生 Windows 上曾經因為另一個理由失效：那邊的環境變數 key 不分
大小寫，而 registry 寫的是 `Path` 不是 `PATH`，照字面找就什麼都找不到，整台
機器看不到 `claude`，也看不到其他任何東西。v0.3.1 起改成按 Windows 自己的
規則、不分大小寫地讀。

### 終端機輸出是 bytes，不是字串

PTY 的讀取邊界落在核心決定的位置。在 Rust 端對每個 chunk 做 UTF-8 解碼，會把
任何跨越邊界的多位元組字元換成 U+FFFD，而 TUI 滿是 3 bytes 的框線字元，畫面
就會沿著 chunk 邊界裂開。所以輸出以 base64 傳遞，交給 xterm 自己有狀態的解碼
器把邊界縫回去。

同理，`lineHeight` 必須正好是 1。大於 1 會在列與列之間留下空隙，框線字元就接
不起來。

### PTY 開始輸出的時間早於 pane 掛載

PTY 一 spawn 就開始吐 bytes，但顯示它的 pane 要等下一次 render 才存在。中間的
輸出，對 Claude Code 來說是整個開場畫面，就會發給沒有人，pane 一片空白。

所以 Rust 端為每個 session 保留一份有上限的 scrollback 與序號。pane 掛載時：
先訂閱（才不會漏），再取快照，然後寫入快照、接著只重播序號比快照新的即時
chunk。順序反過來會漏掉中間的；不比對序號則會寫兩次。

同一套協定也正是「接回被 tmux 扛住的 session」能成立的原因：晚到的 pane 就是
晚到的 pane，晚一次 render 和晚一次 app 重啟並無不同。

### 為什麼是 PTY 而不是 Agent SDK

先做過 SDK 版本：結構化事件、原生訊息串與工具卡片、`canUseTool` 攔截權限請求
彈原生對話框。功能更多，但**畫面就不是終端機了**。既然目標是「跟終端機一樣」，
PTY 是唯一能保證這件事的做法，因為 TUI 自己畫，我們只負責把 bytes 搬過去。

SDK 版本的程式碼收在 `src-tauri/parked/`（Node 那半在 `sidecar/`），沒有刪掉。
如果之後需要攔截而不只是承載工具呼叫，例如無人值守的背景模式或政策層，那份
程式碼是可用的起點。

---

## 已知限制

- 收尾就到「合併」與「開 PR」為止。PR 的 review、留言、CI 紅綠、合併按鈕都
  不做。那是另一個大得多的工具，硬做只會把這裡最深的東西稀釋掉
- 狀態偵測對 Claude Code 與 Codex 有效。其他 CLI 沒有等價的 hook 機制，會
  顯示為「執行中 / 已關閉」而已。首則 prompt 也只對這兩支自動送出，其他
  agent 會把組好的 prompt 顯示出來讓你自己貼（見上）
- 第一次在某個目錄開 session 時，兩支 CLI 都會問你信不信任這個資料夾。這是
  它們原本的行為，刻意不繞過。**每個 attempt 都是新目錄，所以每個 attempt
  都會遇到一次** —— 而第一個 Codex session 還會請你審核一次它的 hooks，
  一台機器一次（見「兩支實測過的 agent」）
- Codex 沒有閒置提示事件，所以 Codex 的卡片會直接從「執行中」跳到「該你了」，
  不會經過 Claude Code 卡片會顯示的「等待輸入」。沒有任何東西回報得出來的
  狀態，這張桌子不會自己發明
- scrollback 不持久化，跟真的終端機一樣。對話歷史由 agent 自己存（Claude Code
  在 `~/.claude/projects/`、Codex 在 `~/.codex/sessions/`），重開時靠那支 CLI
  自己的 resume 接回去
- **設定 outcome 是終局動作**：worktree 會被移除，所以那個 attempt 不再有活的
  TUI。留下來的是時間軸與一份凍結的 diff。superseded 的 attempt 也一樣，
  「保留可回看」指的是唯讀回看，不是還能跳進去打字
- 「活得比 app 久」在任何有 `tmux` 的世界都成立，也僅限於此。沒有 tmux 的
  distro 或主機維持原本的行為：app 一關卡片就停，要按繼續。不會有任何東西
  被替你裝上去
- 被扛住的 session 在它的 agent 送出下一個 hook 事件之前都顯示為
  **執行中，尚未回報**；如果那個 agent 正停在提示符前等人，那可能要等到你
  打字為止
- **跨多個 repo 的合併不是原子的，也不假裝是。** 每一條拒絕條件都在任何一個
  repo 被動到之前就先問完全部，這讓常見的那種情況 —— 有一邊忘了 commit ——
  變回一個什麼都沒改變的當面拒絕。但第一個落地之後，第二個仍可能因為衝突失
  敗；那時候會如實報告：哪些已經進去了、attempt 不關、worktree 不收。git 沒
  有跨 repo 的交易，而假造出一個交易的外觀，比把話說清楚更糟
- 同一張卡上的 repo 必須在同一個世界。attempt 的 worktree 共用一個資料夾，
  而資料夾跨不過通往 WSL distro 或 SSH host 的那道門，所以混世界的卡描述的
  是一個不可能存在的工作區，建卡時就會被拒絕
- 一個所有卡片都被刪掉的世界，它的 socket 會留到你下次在那裡開卡片為止。
  連上一台 SSH host 就是對它開一條連線，為了整理而開一條沒人要求的連線，
  比在我們自己的目錄裡留幾個檔案更糟

---

## 升級

資料庫在第一次啟動時往前遷移，每一步各自一個 transaction，所以中途失敗會停在
最後一個完整套用的版本，而不是卡在下一個版本的半路上。不需要你做任何事。

**反方向 —— 退版 —— 是走不通的那一邊。** 新版寫過的資料庫，舊版會拒絕打開；
它會把話說出來然後停下，而不是往一個它看不懂的形狀裡寫東西：

    database is at schema version 6, but this build understands 5.
    It was written by a newer Marol.

這個拒絕本身就是功能：安靜地弄丟一個看板，比一個開不起來的 app 更糟。但它也
表示退版需要把舊的資料庫拿回來 —— 所以**在替換掉任何東西之前，app 會自己先
複製一份**（見下）。自己手動覆蓋安裝時，這份備份才回到你手上：把 `marol.db`
從狀態目錄複製出來，設定面板的診斷區會寫出它在這台機器上的確切路徑。

### 就地更新

設定 → 更新每天向 GitHub 問一次最新的版本號，有新的就在側邊欄角落放一個點。
按下按鈕會下載、換掉執行檔、重啟進新版。不經過瀏覽器、不經過下載資料夾、
沒有安裝精靈。

它刻意做的四件事：

- **先複製資料庫**，放到原檔旁邊的 `marol.db.before-<版本>`，用 `VACUUM INTO`
  而不是複製檔案 —— 這個資料庫跑在 WAL 模式，磁碟上那個檔案不是它的全部。
  這份複本正是讓上面那道單向門還能推回去的東西，所以複製失敗會中止更新，
  而不是記一行 log 然後照做。
- **它會算出重啟的代價，單位是 agent。** 所在世界有 `tmux` 接著的 session 會被
  detach 然後在下次交回來；沒有的就結束。第二個數字決定按鈕會不會變成
  「結束它們並更新」，而在原生 Windows —— 那裡沒有 tmux 可以當 holder ——
  它就是你所有正在跑的 agent。
- **在 `.deb` 和 `.rpm` 上它會拒絕。** 那些屬於當初安裝它們的套件管理員，
  管理員自己記著它擁有哪些檔案。那裡面板會直說，並改成給你 releases 頁。
  AppImage 可以換掉自己，跟 macOS 和 Windows 一樣算自足。
- **它不會自己動。** 檢查是 app 的事；下載和重啟等人。沒有安靜替換，也沒有
  「10 秒後重啟」。

檢查可以在同一個面板關掉。它不會送出這台機器的任何資料 —— 就是瀏覽器打開
releases 頁時發的同一個請求 —— 但它是 Marol 唯一一個為自己發出的對外請求，
而這種宣稱應該要能被關掉來驗證。

**沒有金鑰的 build 什麼都做不了**，並且會在按鈕原本該在的位置說出來。
見[更新簽章](#更新簽章)。

---

## 從 AgentDesk 升上來

這個 app 以前叫 AgentDesk。更新會把東西整批帶過來：

- **看板跟著過來。** 狀態目錄在第一次啟動時改名——資料庫、機器 id、記住的
  hook endpoint 與隧道 port。沒有任何外面的東西指進去，所以改名就只是改名。
  如果新名字的目錄已經存在，那個贏，而且絕不會被蓋過去
- **worktree 原地不動**，留在 `~/.agentdesk/worktrees`，而且只要那個目錄還在，
  桌子就繼續用它。這些路徑同時寫在開出它們的 attempt 資料列裡**以及**各個 repo
  自己的 git 管理檔裡；搬動會把兩端一起弄斷。新安裝拿到的是
  `~/.marol/worktrees`，而這一台也會在最後一棵舊樹還回去之後自己換過去
- **tmux 正扛著的 agent 繼續跑，而且是被接回去、不是被重開。** 它們的 socket
  在舊名字底下；去要新名字會在同一個 worktree 裡開出第二個 agent
- **`.agentdesk/config.json` 與 `$AGENTDESK_*` 繼續有效**——見
  [讓 worktree 開箱能跑](#讓-worktree-開箱能跑)

---

## 授權

Apache-2.0，全文在 [LICENSE](LICENSE)。
