<p align="center"><img src="app/src-tauri/icons/128x128.png" width="96" alt="Biểu tượng Loah"></p>

<h1 align="center">Loah</h1>

<p align="center">
  <b>Nói thay vì gõ, trên macOS, Windows và Linux.</b><br>
  Lời bạn nói được viết thẳng vào ô chữ đang chọn, và giọng nói được nhận dạng ngay trên máy của bạn.<br>
  Loah còn ghi và tóm tắt cuộc họp, và cho trợ lý AI của bạn đọc lại chúng.
</p>

<p align="center">
  <a href="https://github.com/henryonai/loah/releases/latest"><b>Tải về</b></a> ·
  <a href="#tải-về-và-cài-đặt">Cài đặt</a> ·
  <a href="#cách-dùng">Cách dùng</a> ·
  <a href="#nâng-cấp-từ-dictate">Nâng cấp từ Dictate</a> ·
  <a href="#quyền-riêng-tư">Quyền riêng tư</a> ·
  <a href="README.md">English</a>
</p>

<p align="center">
  <img alt="macOS 13 trở lên" src="https://img.shields.io/badge/macOS-13%2B-111111?logo=apple">
  <img alt="Windows 10 và 11" src="https://img.shields.io/badge/Windows-10%20%7C%2011-111111">
  <img alt="Linux, Ubuntu 24.04 trở lên" src="https://img.shields.io/badge/Linux-Ubuntu%2024.04%2B-111111?logo=linux&logoColor=white">
  <a href="LICENSE"><img alt="Giấy phép MIT" src="https://img.shields.io/badge/license-MIT-1D3557"></a>
  <a href="https://github.com/henryonai/loah/releases/latest"><img alt="Bản mới nhất" src="https://img.shields.io/github/v/release/henryonai/loah?color=FF7A59"></a>
</p>

<p align="center">
  <b>Loah là tên mới của Dictate</b>, từ phiên bản 2.1.0. Cài đặt, model và thư viện của bạn được mang theo:
  xem <a href="#nâng-cấp-từ-dictate">Nâng cấp từ Dictate</a>.
</p>

## Tính năng

**Đọc vào bất kỳ ô chữ nào.** Bấm vào ô chat, email hay tài liệu rồi nhấn phím tắt (`Ctrl+Alt+Space`, trên Mac là `⌃⌥Space`; đổi được). Chữ hiện cạnh con trỏ trong lúc bạn nói. Nói xong, Loah viết đoạn chữ vào ô, và nếu bạn đã cài một AI thì sửa chữ nghe nhầm và dấu câu trước.

**Viết lại câu vừa nói.** Chip nhỏ cạnh đoạn chữ có menu **⋯** để có bản khác: ngắn hơn, thành email, gạch đầu dòng, bản dịch…

**Ghi và tóm tắt cuộc họp.** Quay màn hình, một cửa sổ hoặc chỉ ghi âm, hoặc thêm file âm thanh, file video, hay link (YouTube và các trang khác yt-dlp hỗ trợ). Loah viết transcript, tách người nói (Speaker 1, 2…) và, khi đã cài AI, viết bản tóm tắt kèm các quyết định và việc cần làm. Bấm mốc thời gian để nhảy tới đúng đoạn đó, và hỏi thêm về bản ghi.

**Hỏi trợ lý AI về cuộc họp.** [MCP server](#hỏi-claude-về-cuộc-họp-mcp) có sẵn trong app (`loah-mcp`) cho Claude Code, Claude Desktop hay bất kỳ MCP client nào đọc các bản ghi của bạn.

Giọng nói được nhận dạng trên máy bằng [Whisper](https://github.com/ggml-org/whisper.cpp): giọng của bạn không rời khỏi máy trừ khi bạn chọn dùng API transcript. Giao diện có tiếng Việt và tiếng Anh (Cài đặt → Chung → *Ngôn ngữ giao diện*; mặc định theo hệ thống).

## Tải về và cài đặt

Tải bộ cài cho hệ thống của bạn ở mục [Releases](https://github.com/henryonai/loah/releases/latest). `<version>` trong tên file là số phiên bản, ví dụ `2.1.0`.

| Hệ thống | Bộ cài | File checksum | Yêu cầu |
|---|---|---|---|
| macOS, chip Apple (M1 trở lên) | `Loah_<version>_aarch64.dmg` | `Loah_<version>_aarch64.dmg.sha256` | macOS 13 trở lên |
| macOS, chip Intel | `Loah_<version>_x64.dmg` | `Loah_<version>_x64.dmg.sha256` | macOS 13 trở lên |
| Windows, 64-bit | `Loah_<version>_x64-setup.exe` | `Loah_<version>_x64-setup.exe.sha256` | Windows 10 hoặc 11 |
| Linux, 64-bit | `Loah_<version>_amd64.deb` | `Loah_<version>_amd64.deb.sha256` | glibc 2.39 trở lên (Ubuntu 24.04 trở lên) |

Các bộ cài chưa được ký: đọc [Bản phát hành chưa ký](#bản-phát-hành-chưa-ký) trước khi mở bộ cài đầu tiên.

### macOS

1. Mở file `.dmg` rồi kéo **Loah** vào **Applications**. Hãy cài vào đó trước khi mở lần đầu: tính năng *Tự mở Loah khi khởi động máy* nhớ vị trí của app.
2. Mở app. macOS chặn lần mở đầu tiên của app chưa ký; cách cho phép nằm ở [Bản phát hành chưa ký](#bản-phát-hành-chưa-ký).
3. Loah chạy trên menu bar (không có icon ở Dock). Làm tiếp theo mục [Lần chạy đầu tiên](#lần-chạy-đầu-tiên).

### Windows

1. Chạy file `.exe`. Bộ cài chỉ cài cho tài khoản của bạn, không cần quyền administrator, và hỏi có mở Loah khi cài xong không. Thư viện Visual C++ mà app cần đã đi kèm. Windows 11 có sẵn WebView2 mà cửa sổ của app dùng; trên Windows 10, nếu máy chưa có thì bộ cài tải về, nên cần có mạng.
2. Windows SmartScreen cảnh báo bộ cài chưa ký: cách đi tiếp nằm ở [Bản phát hành chưa ký](#bản-phát-hành-chưa-ký).
3. Loah chạy ở vùng thông báo của thanh taskbar (bấm **^** nếu chưa thấy icon). Làm tiếp theo mục [Lần chạy đầu tiên](#lần-chạy-đầu-tiên).

### Linux

File `.deb` dành cho Ubuntu 24.04 trở lên. Nó được build trên Ubuntu 24.04 nên cần glibc 2.39 trở lên; các hệ Debian khác có glibc như vậy (ví dụ Debian 13) chắc cũng chạy được nhưng chưa được thử. Cài bằng `apt`, apt sẽ tải luôn các thư viện Loah cần (thay `<version>` bằng số phiên bản của file bạn đã tải, ví dụ `2.1.0`):

```sh
sudo apt install ./Loah_<version>_amd64.deb
```

Gói cài đặt đặt `loah` và `loah-mcp` vào `/usr/bin`, thư viện riêng của app vào `/usr/lib/loah`. Gói đề xuất thêm `gstreamer1.0-libav` và `gstreamer1.0-plugins-good` (để phát bản ghi), `xdg-desktop-portal` (để quay màn hình) và `pulseaudio-utils` (để ghi âm thanh của máy); `apt` cài các gói này cùng, trừ khi bạn bảo nó đừng. Mở Loah từ menu ứng dụng hoặc bằng lệnh `loah`. Nếu máy đang có gói `dictate`, gói này sẽ thay nó: xem [Nâng cấp từ Dictate](#nâng-cấp-từ-dictate). Icon trên khay cần desktop có hiện app indicator (GNOME cần extension AppIndicator). Mục [Đọc chính tả trên từng hệ điều hành](#đọc-chính-tả-trên-từng-hệ-điều-hành) nói Wayland cần gì.

### Bản phát hành chưa ký

Loah chưa có chứng chỉ ký, nên bộ cài macOS và Windows chưa được ký và chưa được notarize. Hệ thống sẽ cảnh báo ở lần mở đầu tiên, đó là chuyện bình thường. Hãy [kiểm tra bản tải về](#kiểm-tra-bản-tải-về) để chắc rằng file đúng là file trên trang phát hành.

- **macOS 15 trở lên:** mở Loah một lần; macOS báo không kiểm tra được app. Vào **System Settings → Privacy & Security**, kéo xuống dòng báo về Loah, bấm **Open Anyway** rồi xác nhận. (Trên macOS 13 và 14 còn có cách giữ Control rồi bấm vào app và chọn **Open**.) Nếu macOS báo app "bị hỏng" (damaged), hãy gỡ cờ đánh dấu file tải về rồi mở lại:

  ```sh
  xattr -dr com.apple.quarantine /Applications/Loah.app
  ```

- **Windows:** SmartScreen hiện "Windows protected your PC". Bấm **More info**, rồi **Run anyway**. Trên Windows 11 đang bật **Smart App Control** (bản Windows 11 mới cài có thể đã bật sẵn; xem ở Settings → Privacy & security → Windows Security → App & browser control → Smart App Control settings) thì không có nút đó: nó chặn thẳng bộ cài chưa ký. Tắt nó đi thì bộ cài chạy được, nhưng Microsoft không cho bật lại nếu không đặt lại (reset) hoặc cài lại Windows, nên hãy cân nhắc trước khi tắt trên máy quan trọng.
- **Linux:** không cần cho phép gì.

### Kiểm tra bản tải về

Mỗi bộ cài có một file checksum ngay cạnh trên trang phát hành, một dòng dạng `<hash>  <tên file>`. Tải cả hai vào cùng một thư mục rồi kiểm tra. Trong mỗi lệnh, hãy thay `<version>` bằng số phiên bản của các file bạn đã tải (ví dụ `2.1.0`):

```sh
shasum -a 256 -c Loah_<version>_aarch64.dmg.sha256
```

trên macOS (dùng tên file `.dmg` bạn đã tải: `_aarch64` cho chip Apple, `_x64` cho chip Intel),

```sh
sha256sum -c Loah_<version>_amd64.deb.sha256
```

trên Linux, và trên Windows, trong PowerShell (với số phiên bản của file `.exe` bạn tải ở cả hai tên file), lệnh này in `True` khi file đúng:

```powershell
(Get-FileHash -Algorithm SHA256 .\Loah_<version>_x64-setup.exe).Hash -eq (Get-Content .\Loah_<version>_x64-setup.exe.sha256).Split(' ')[0]
```

Checksum lấy từ cùng một trang chỉ cho biết file tải về còn nguyên vẹn, không cho biết ai làm ra nó. Các bộ cài được [GitHub Actions](https://github.com/henryonai/loah/actions) build từ mã nguồn của chính bản phát hành đó.

### Cập nhật

Chưa có cập nhật tự động: hãy tải bộ cài mới rồi cài đè lên bản cũ (trên Mac, thoát Loah rồi kéo app mới đè lên app cũ; trên Linux, `sudo apt install` file `.deb` mới). Cài đặt, model và thư viện cuộc họp của bạn vẫn nằm nguyên chỗ cũ. Các bản build chưa ký, nên sau khi cập nhật macOS có thể xem app là một app khác và hỏi lại các quyền: xem [Lần chạy đầu tiên](#lần-chạy-đầu-tiên). Bạn đang dùng Dictate? Hãy đọc [Nâng cấp từ Dictate](#nâng-cấp-từ-dictate) trước.

### Gỡ cài đặt

Hãy thoát Loah trước (biểu tượng trên khay hoặc thanh menu, **Quit**). Các bản ghi của bạn, trong thư mục `Loah Meetings` của Documents (hoặc trong `Dictate Meetings`, nếu thư viện của bạn vẫn nằm ở đó từ trước), không bao giờ bị xóa.

- **macOS:** kéo `Loah.app` từ Applications vào Thùng rác. Cài đặt, model và các đoạn đọc đã lưu vẫn nằm trong `~/Library/Application Support/Loah`, còn khóa API nằm trong Keychain Access (các mục có service là `Loah`).
- **Windows:** Settings → Apps → Installed apps → Loah → Uninstall. Trình gỡ chỉ gỡ chương trình (file và lối tắt), không đụng đến dữ liệu của bạn, **và ô "Delete the application data" cũng không đổi điều đó:** nó chỉ xóa các thư mục đặt theo mã định danh của app (`io.github.henryonai.loah`). Các model giọng nói đã tải và các đoạn đọc đã lưu vẫn nằm trong `%LOCALAPPDATA%\Loah`, cài đặt trong `%APPDATA%\Loah`, còn khóa API trong Credential Manager (`default.Loah` và `transcribe.Loah`). Muốn xóa luôn, sau khi gỡ, chạy trong PowerShell:

  ```powershell
  Remove-Item -Recurse -Force "$env:LOCALAPPDATA\Loah", "$env:APPDATA\Loah"
  cmdkey /delete:default.Loah
  cmdkey /delete:transcribe.Loah
  ```

- **Linux:** `sudo apt remove loah` gỡ chương trình. Cài đặt (`~/.config/Loah`), model và các đoạn đọc đã lưu (`~/.local/share/Loah`) vẫn còn, khóa API trong Secret Service (ứng dụng Passwords and Keys) cũng vậy. Muốn xóa luôn các file:

  ```sh
  rm -rf ~/.config/Loah ~/.local/share/Loah
  ```

Dữ liệu của Dictate, nếu bạn từng dùng, không phải việc của Loah để xóa: xem [Nâng cấp từ Dictate](#nâng-cấp-từ-dictate) để biết những gì còn lại và cách xóa hẳn một key.

## Nâng cấp từ Dictate

Loah là Dictate dưới một cái tên mới, từ phiên bản 2.1.0. Với hệ điều hành, đây là một app mới, có tên và mã định danh riêng (`io.github.henryonai.loah`), nên bộ cài sẽ cài nó cạnh Dictate chứ không đè lên (trên Linux thì gói mới thay gói cũ: xem bên dưới). Ở lần mở đầu tiên, Loah mang dữ liệu của bạn sang, và không bao giờ sửa hay xóa bất cứ thứ gì của Dictate, nên bạn vẫn quay lại Dictate được cho tới khi tự tay gỡ nó. Tóm tắt:

1. Cài Loah và mở một lần: Loah sẽ chép những gì bạn đã có.
2. Trỏ MCP client của bạn sang Loah.
3. Thoát Dictate và gỡ nó.

Khi Dictate đang chạy, hoặc được đặt tự mở khi khởi động máy, Cài đặt → Chung hiện một thông báo kèm nút mở mục này: hai app dùng chung phím tắt và thư mục cuộc họp, nên chỉ chạy một trong hai.

### Loah chép gì từ Dictate

Ở lần mở đầu tiên, Loah chép bốn thứ mà Dictate 2.0 đã lưu vào các thư mục của riêng mình ([vị trí của chúng](#cài-đặt)):

- `settings.json`, cài đặt của bạn;
- `whisper-models`, các model Whisper bạn đã tải, và `diarization-models`, các model tách người nói: trên cùng một ổ đĩa, nếu hệ thống file cho phép thì chúng được liên kết cứng (hard link) nên không tốn thêm chỗ, còn không thì được chép;
- `Dictations`, các đoạn đọc đã lưu.

Ngoài ra không chép gì khác (các file mà app Swift để lại trong cùng thư mục vẫn nằm yên). Việc chép chỉ làm một lần, sau đó hai app giữ cài đặt riêng: đổi gì ở Dictate thì Loah không thấy. Nếu Dictate đã bật tự mở khi khởi động máy, Loah bật *Tự mở Loah khi khởi động máy* cho chính nó ở lần mở đầu tiên đó; mục khởi động của Dictate vẫn còn cho tới khi bạn gỡ Dictate, nên tới lúc đó cả hai cùng chạy.

Nếu một mục chép không được (file đang được dùng, đĩa đầy), Loah vẫn đọc bản của Dictate cho mục đó, ở chế độ chỉ đọc, báo trong Cài đặt và thử lại ở lần mở sau. Những gì chính Loah lưu (cài đặt, model nó tải, đoạn đọc mới) luôn nằm trong thư mục của Loah. Vì vậy, khi việc chép các đoạn đọc đã lưu bị lỗi, mục **Mở thư mục đọc chính tả** trên menu khay mở thư mục `Dictations` của Dictate, nhưng các đoạn đọc mới vẫn được lưu vào thư mục `Dictations` của Loah.

**macOS:** hệ thống coi Loah là một app khác, nên phải cấp lại quyền Microphone, Accessibility và Screen Recording ([Lần chạy đầu tiên](#lần-chạy-đầu-tiên)), và hướng dẫn cấp quyền Accessibility sẽ hiện lại. Khi Dictate đã được gỡ, hãy xóa nó khỏi các danh sách quyền đó.

### Thư viện, biến môi trường và khóa API

- **Thư viện cuộc họp.** Nếu bạn có thư mục `Dictate Meetings` trong Documents mà chưa có `Loah Meetings`, Loah và `loah-mcp` tiếp tục dùng `Dictate Meetings` tại chỗ: không có gì bị di chuyển hay đổi tên. Bản cài mới dùng `Loah Meetings`.
- **Biến môi trường.** Tên hiện nay là `LOAH_MEETINGS_DIR` và `LOAH_FFMPEG`. `DICTATE_MEETINGS_DIR` và `DICTATE_FFMPEG` vẫn được đọc khi biến mới chưa đặt, nên cấu hình MCP hay shell profile đang đặt chúng vẫn chạy.
- **API key.** Các key được lưu trong kho credential của hệ thống với service `Loah` (trên Windows là các mục `default.Loah` và `transcribe.Loah`). Lần đầu Loah cần một key mà nó chưa có, nó chép key của Dictate (service `Dictate`; trên Windows là `default.Dictate` và `transcribe.Dictate`) và để nguyên mục của Dictate. Trên Mac, Keychain có thể hỏi Loah có được dùng mục mà Dictate đã lưu không: hãy chọn **Cho phép (Allow)**. Nếu bạn chọn **Từ chối (Deny)**, Loah coi như chưa có key (và báo cần thêm key) rồi hỏi lại ở lần mở sau; bạn cũng có thể nhập lại key trong Cài đặt. Xóa một key trong Cài đặt → *Tài khoản & API key* thì key bị xóa ở Loah và mục của Dictate vẫn nguyên.

Muốn xóa hẳn một key bằng tay, hãy xóa **cả hai** mục, của Loah và của Dictate: nếu để lại mục của Dictate thì lần tới cần key, Loah sẽ chép nó trở lại. Trên Windows, trong PowerShell:

```powershell
cmdkey /delete:default.Loah
cmdkey /delete:default.Dictate
cmdkey /delete:transcribe.Loah
cmdkey /delete:transcribe.Dictate
```

Trên Mac, xóa các mục có service là `Loah` và `Dictate` trong Keychain Access; trên Linux, xóa hai mục đó trong ứng dụng Passwords and Keys.

### Trỏ MCP client sang Loah rồi gỡ Dictate

MCP server có đường dẫn mới. Hãy giữ nguyên tên mà client của bạn gọi nó, `dictate`, thì tên các tool mà prompt của bạn đang dùng vẫn như cũ; với một thiết lập mới thì dùng tên `loah` ([Hỏi Claude về cuộc họp](#hỏi-claude-về-cuộc-họp-mcp)).

| Hệ thống | Server của Dictate | Server của Loah |
|---|---|---|
| macOS | `/Applications/Dictate.app/Contents/MacOS/dictate-mcp` | `/Applications/Loah.app/Contents/MacOS/loah-mcp` |
| Windows | `%LOCALAPPDATA%\Dictate\dictate-mcp.exe` | `%LOCALAPPDATA%\Loah\loah-mcp.exe` |
| Linux | `/usr/bin/dictate-mcp` | `/usr/bin/loah-mcp` |

- **macOS.** Trỏ lại Claude Code (mục đã có thì phải xóa trước khi thêm lại). Nếu lệnh đầu báo không có MCP server nào tên `dictate`, cứ chạy tiếp dòng sau:

  ```sh
  claude mcp remove dictate
  claude mcp add --scope user dictate -- /Applications/Loah.app/Contents/MacOS/loah-mcp
  ```

  Với Claude Desktop, đổi `command` của mục `dictate` trong `claude_desktop_config.json` sang đường dẫn mới rồi khởi động lại Claude. Sau đó thoát Dictate (biểu tượng trên thanh menu → Quit) và kéo `/Applications/Dictate.app` vào Thùng rác: bộ cài của Loah không đụng tới nó.
- **Windows.** Loah được cài cạnh Dictate (bộ cài cho một tài khoản không thể tự gỡ một chương trình khác một cách an toàn), và `%LOCALAPPDATA%\Dictate\dictate-mcp.exe` vẫn chạy được cho tới khi bạn gỡ Dictate. Trỏ lại Claude Code, trong PowerShell:

  ```powershell
  claude mcp remove dictate
  claude mcp add --scope user dictate -- "$env:LOCALAPPDATA\Loah\loah-mcp.exe"
  ```

  Với Claude Desktop, đổi `command` của mục `dictate` sang đường dẫn mới (mỗi dấu gạch chéo ngược viết hai lần) rồi khởi động lại Claude. Sau đó thoát Dictate và gỡ nó: Settings → Apps → Installed apps → Dictate → Uninstall.
- **Linux.** Cài file `.deb` của Loah sẽ thay luôn gói `dictate`, còn `/usr/bin/dictate-mcp` và `/usr/bin/dictate` vẫn chạy được: đó là hai script nhỏ chỉ việc khởi động `loah-mcp` và `loah`, nên MCP client đã thiết lập cho Dictate và phím tắt gán cho `dictate --toggle` vẫn hoạt động như cũ. Hai script này được giữ trong suốt các bản 2.x và sẽ bị bỏ ở một bản chính (major) sau, ghi chú phát hành sẽ báo trước. Vì vậy, khi tiện, bạn nên trỏ client sang `loah-mcp`:

  ```sh
  claude mcp remove dictate
  claude mcp add --scope user dictate -- loah-mcp
  ```

Gỡ Dictate không xóa dữ liệu của nó: các thư mục của nó (giống thư mục của Loah nhưng tên là `Dictate`; xem [Gỡ cài đặt](#gỡ-cài-đặt)) và các key vẫn còn trên máy. Loah đã có bản riêng, nên bạn xóa lúc nào cũng được, sau khi Cài đặt không còn thông báo nào về mục chép bị lỗi. Khi muốn xóa key thì xóa của cả hai app cùng lúc (xem ở trên).

### Nếu bạn dùng Dictate bản Swift (1.x) trên macOS

App Swift cũ là một chương trình khác, trùng tên với app mà Loah thay thế. Loah được cài cạnh nó và không dùng file nào của nó:

1. **Thoát app cũ trước** (biểu tượng trên menu bar → Quit).
2. Cài Loah rồi cấp các quyền ([Lần chạy đầu tiên](#lần-chạy-đầu-tiên)). Cài đặt và API key của app cũ cũng không được chuyển sang: hãy nhập lại trong Cài đặt.
3. Trỏ MCP client sang Loah như ở trên, rồi kéo `Dictate.app` cũ vào Thùng rác.

Các file riêng của app cũ vẫn nằm nguyên chỗ cũ và Loah không dùng chúng: các model WhisperKit trong `~/Library/Application Support/Dictate/Models` và các bản ghi âm đọc chính tả của nó trong thư mục `Recordings` cạnh đó. Bạn có thể xóa cả hai thư mục để lấy lại dung lượng. Những gì giữ nguyên: thư viện của bạn (thư mục `Dictate Meetings` trong Documents cùng định dạng, các bản ghi của app cũ mở được như thường). Những gì không còn: Apple Speech và WhisperKit. Giọng nói được nhận dạng bằng các model Whisper, bạn tải lại trong Cài đặt.

## Lần chạy đầu tiên

1. **Quyền truy cập (macOS).** Cài đặt → Chung → *Quyền truy cập* cho thấy từng quyền, kèm nút xin quyền hoặc mở đúng mục của System Settings:

   | Quyền | Dùng để | Khi nào |
   |---|---|---|
   | Microphone | Đọc chính tả và micro khi quay | Hỏi ngay lần đọc chính tả đầu tiên |
   | Accessibility | Tìm ô chữ bạn đang chọn và dán chữ vào đó. Chưa cấp thì chữ hiện trong thẻ để bạn sửa và sao chép | Cài đặt → Chung → Quyền truy cập → *Cấp quyền…* |
   | Screen Recording | Quay màn hình và ghi âm thanh của máy, kể cả khi chỉ ghi âm | Hỏi ở lần quay màn hình đầu tiên; bật xong cần mở lại Loah |

   Windows và Linux không hỏi quyền nào lúc cài: xem [Đọc chính tả trên từng hệ điều hành](#đọc-chính-tả-trên-từng-hệ-điều-hành) để biết mỗi hệ cần gì.

2. **Chọn model Whisper.** Cài đặt → *Nhận dạng giọng nói* → *Model Whisper*: bấm **Tải** ở model được đánh dấu *Đề xuất* (tuỳ theo dung lượng RAM của máy; **Tiny** và **Base** kém tiếng Việt, từ **Small** trở lên tốt hơn). Model chỉ tải một lần, từ Hugging Face, sau đó giọng nói được nhận dạng offline. Chưa có model thì khi đọc chính tả app sẽ báo.
3. **Tuỳ chọn: thêm key OpenRouter** để sửa chữ khi đọc chính tả và để tóm tắt. Mặc định Loah dùng [OpenRouter](https://openrouter.ai) với model `deepseek/deepseek-v4.1-flash`, nên bạn không phải nhập URL hay tên model: tạo key tại [openrouter.ai/keys](https://openrouter.ai/keys), rồi vào Cài đặt → *Tài khoản & API key* → *AI*, dán key vào và bấm **Kiểm tra**. Nếu *API transcript* cũng chọn OpenRouter thì không cần key thứ hai: nó dùng chung key này.

   Muốn dùng dịch vụ khác, chọn ở mục *Nhà cung cấp* rồi nhập base URL, tên một model mà dịch vụ đó có và key của bạn:
   - **Anthropic:** base URL `https://api.anthropic.com`, một model như `claude-haiku-5-5` (nhanh) hoặc `claude-sonnet-5-5`.
   - **Tương thích OpenAI:** OpenRouter với model khác (như `anthropic/claude-haiku-5.5`), OpenAI, hoặc server chạy trên máy như Ollama (`http://localhost:11434/v1`).

   API key được lưu trong kho credential của hệ thống (Keychain, Credential Manager, Secret Service), không bao giờ nằm trong file. Chưa có AI thì đọc chính tả vẫn chạy, chỉ là chữ không được sửa, và bản ghi chỉ có transcript, không có tóm tắt.
4. **Chọn ngôn ngữ đọc:** Cài đặt → *Chung* → *Đọc chính tả* → *Ngôn ngữ* (mặc định là English (US); nói tiếng Việt thì chọn *Tiếng Việt*).
5. **Bấm vào một ô chữ bất kỳ, nhấn phím tắt, nói, rồi nhấn lại lần nữa.**

## Cách dùng

### Đọc chính tả

| Thao tác | Phím |
|---|---|
| Bắt đầu / dừng đọc | `Ctrl+Alt+Space` (trên Mac là `⌃⌥Space`); đổi trong Cài đặt → Chung, nơi cũng có chế độ giữ phím để nói |
| Viết luôn, bỏ qua bước AI sửa | nhấn phím tắt lần nữa khi bong bóng đang “Đang sửa: …” |
| Kiểu khác cho đoạn vừa viết | bấm **⋯** trên chip cạnh đoạn đó |
| Huỷ khi đang nói | `Esc` |
| Trên thẻ (khi không có ô chữ nào đang chọn): các kiểu sửa | `Ctrl+1–5` (trên Mac là `⌘1–5`) |

**Viết vào ô chữ** (Cài đặt → Chung → *Tự viết vào ô chữ đang chọn*, bật sẵn): trong lúc nói, một bong bóng chữ live nằm cạnh con trỏ. Nói xong, Loah dùng AI của bạn để sửa lỗi và dấu câu (kiểu mặc định ở tab *AI sửa & tóm tắt*) rồi dán kết quả vào ô. Sau đó một chip nhỏ nằm cạnh đoạn chữ vài giây: menu **⋯** của nó viết lại đoạn chữ ngay tại chỗ (rút gọn, thành email, gạch đầu dòng, trang trọng, thành prompt cho AI, dịch sang tiếng Anh hoặc tiếng Việt, bản gốc, yêu cầu riêng của bạn). Gõ phím, bấm chuột chỗ khác hoặc chuyển app thì chip biến mất và Loah không đụng vào ô nữa.

Khi không có ô chữ nào đang chọn, hoặc khi đã tắt thiết lập đó, một thẻ hiện ra để bạn sửa và sao chép đoạn chữ. Hãy nhập tên riêng và thuật ngữ vào *Từ vựng / ngữ cảnh riêng* (Cài đặt → *AI sửa & tóm tắt*) để AI viết đúng chính tả.

**Đoạn đọc được lưu.** Khi *Lưu file ghi âm + transcript* bật (mặc định), âm thanh và chữ của mỗi lần đọc được giữ trong `Loah/Dictations` ở thư mục dữ liệu cục bộ của bạn (menu trên khay mở được thư mục này). Đoạn đọc không được lưu thì không để lại gì.

#### Đọc chính tả trên từng hệ điều hành

- **macOS:** đọc chính tả cần quyền Microphone và Accessibility (xem [Lần chạy đầu tiên](#lần-chạy-đầu-tiên)). App chưa ký, nên sau khi build lại hoặc cập nhật, có thể phải cấp lại Accessibility: trong System Settings → Privacy & Security → Accessibility, xóa Loah khỏi danh sách rồi thêm lại.
- **Windows:** app chạy với quyền administrator thì app thường không dán vào được: Loah sao chép chữ, thẻ báo như vậy, và bạn nhấn `Ctrl+V`. Nếu micro chỉ thu được im lặng, kiểm tra Settings → Privacy & security → Microphone và cho phép các app desktop dùng micro.
- **Linux:** X11 dùng bình thường. Trên Wayland không app nào được nhấn phím trong app khác, nên hãy cài `wtype` (desktop wlroots như Sway, và Hyprland) hoặc `ydotool` để Loah dán được; không có cái nào thì chữ được sao chép và bạn nhấn `Ctrl+V`. Phím tắt đến từ GlobalShortcuts portal của desktop (KDE Plasma, GNOME 48 trở lên, Hyprland); nơi nào không có (Sway và các desktop wlroots khác), hãy gán một phím trong cài đặt của desktop để chạy `loah --toggle`.

`loah --toggle` làm đúng việc của một lần nhấn phím tắt, kể cả khi app chưa chạy, và `loah --record` làm vậy với việc quay. Chương trình này không nằm trong đường dẫn tìm kiếm của mọi hệ thống:

| Hệ thống | Chương trình cần chạy |
|---|---|
| macOS | `/Applications/Loah.app/Contents/MacOS/loah` |
| Windows | `%LOCALAPPDATA%\Loah\loah.exe` |
| Linux | `loah` |

### Cuộc họp, video và link

Mục **Cuộc họp…** trên menu khay mở thư viện: thêm một bản ghi thì Loah tạo transcript, tách người nói (Speaker 1, 2…) và, khi đã cài AI, viết bản tóm tắt gồm ý chính, quyết định, việc cần làm tick được và các chương nhảy tới đúng đoạn trong bản ghi. Bạn hỏi được về bản ghi, đổi tên người nói, xuất Markdown, hoặc chuyển bản ghi vào Thùng rác (Recycle Bin).

- **Thêm bản ghi.** Chọn file âm thanh hoặc video, kéo thả vào cửa sổ, hoặc dán một link (YouTube hay trang nào yt-dlp hỗ trợ) bằng `Ctrl+V` / `⌘V` ở bất kỳ đâu trong cửa sổ. **File bạn thêm không được chép hay di chuyển:** bản ghi vẫn trỏ về file gốc, nên đừng chuyển file đi. Nếu file bị chuyển đi hoặc xóa, transcript và tóm tắt vẫn còn, còn phần phát sẽ báo file không còn.
- **Thư mục thư viện** là `Loah Meetings` trong thư mục Documents của bạn (thư viện do Dictate tạo vẫn nằm ở `Dictate Meetings`, tại chỗ: xem [Nâng cấp từ Dictate](#nâng-cấp-từ-dictate)), mỗi bản ghi một thư mục đặt theo thời gian bắt đầu, bên trong có `transcript.txt`, `summary.md`, `tasks.json`, `log.txt` và `meta.json`. Thư viện của app Swift cũ có cùng định dạng. Đặt biến `LOAH_MEETINGS_DIR` (dấu `~` ở đầu là thư mục home) trước khi mở app để dùng thư mục khác, ví dụ để dùng thử mà không đụng vào thư viện thật; `loah-mcp` đọc cùng biến này. `DICTATE_MEETINGS_DIR`, tên mà Dictate từng dùng, vẫn được đọc khi `LOAH_MEETINGS_DIR` chưa đặt.
- **Chuyển giọng nói thành chữ.** Mặc định là Whisper trên máy; **API transcript** (OpenRouter, OpenAI, Groq hoặc endpoint của bạn) là lựa chọn cho file và cho đọc chính tả (Cài đặt → *Nhận dạng giọng nói* và *Tài khoản & API key*). Lần đầu tách người nói, Loah tải thêm hai model (khoảng 46 MB).
- **Phát bản ghi trên Linux** cần plugin libav của GStreamer, gói `.deb` đã đề xuất sẵn: `sudo apt install gstreamer1.0-libav gstreamer1.0-plugins-good`. Chưa có thì cửa sổ nói cần cài gì; transcript và tóm tắt vẫn dùng bình thường.

### Quay màn hình

Mục **Quay & tóm tắt…** trên menu khay (và nút **Quay** của cửa sổ Cuộc họp) mở một màn chọn. Chọn thứ cần quay rồi bấm **Bắt đầu quay**; khi bạn dừng, bản ghi đi qua các bước như file được thêm vào: transcript, người nói và, khi đã cài AI, tóm tắt.

- **Mỗi chế độ quay gì.** *Toàn màn hình* là một màn hình, kèm âm thanh của máy và micro của bạn (có nhiều màn hình thì màn chọn cho bạn chọn; mặc định là màn hình chính). *Một cửa sổ* là một cửa sổ, cho tới khi bạn dừng hoặc đóng nó. *Chỉ âm thanh* là âm thanh không có hình. Ở mọi chế độ, âm thanh của máy và micro đều tắt được từng cái (trừ khi cả hai cùng tắt ở *Chỉ âm thanh*). *Chất lượng video* có **Nhẹ** (10 hình/giây, cạnh dài tối đa 1280 px), **Chuẩn** (15, tối đa 1920; mặc định) hoặc **Nét** (30, tối đa 3840). Màn chọn nhớ chế độ, chất lượng và hai nguồn âm thanh cho lần sau.
- **Thanh quay.** Trong lúc quay, một thanh nhỏ ở đầu màn hình hiện thời gian và mức âm của từng nguồn, kèm **Tạm dừng** và **Tiếp tục**, **Dừng & tóm tắt** và **Bỏ bản ghi**. Kéo thanh để đổi chỗ; nó không bao giờ giành focus.
- **Phím tắt.** `Ctrl+Alt+R` (trên Mac là `⌃⌥R`) bắt đầu hoặc dừng quay ở mọi nơi, dùng lựa chọn của lần trước. Khi chế độ lần trước là *Một cửa sổ*, phím tắt mở màn chọn, vì app không nhớ cửa sổ. Trên desktop Wayland không có GlobalShortcuts portal, hãy gán một phím trong cài đặt của desktop để chạy `loah --record`.
- **Khi có sự cố.** Rút tai nghe hoặc micro ra thì bản ghi vẫn tiếp tục với thiết bị mà hệ thống đang dùng (khoảng giữa là im lặng) và thanh quay báo "đã nối lại". Đóng cửa sổ đang quay, mất màn hình hoặc tắt quyền thì bản ghi tự kết thúc, phần đã quay được giữ lại và xử lý, và một thông báo nói lý do. Nếu Loah bị sập (crash) giữa chừng khi đang quay, lần mở sau sẽ hoàn tất bản ghi từ những gì có trên đĩa và mất tối đa khoảng một giây cuối (hình thì hai giây), và log ghi rằng bản ghi đã bị gián đoạn. Còn nếu mất điện thì có thể mất nhiều hơn, vì các file không được ép ghi xuống đĩa bằng `fsync`, nên phần hệ thống chưa kịp ghi sẽ mất. Chọn **Thoát** trên menu khay khi đang quay thì app lưu bản ghi trước (chờ tối đa một phút) rồi mới thoát.
- **Lưu ở đâu.** Bản ghi là một thư mục của thư viện như các bản khác, gồm video (`recording.mp4`, hoặc `recording.webm` trên Linux, nơi VP9 được chọn trước), `audio.m4a` và các file còn lại. Khác với file được thêm vào, bản ghi nằm hẳn trong thư viện.
- **macOS** 13 trở lên. Quay màn hình **và âm thanh của máy, kể cả ở chế độ *Chỉ âm thanh*,** cần quyền Screen Recording (Cài đặt → Chung → Quyền truy cập có hiện; bật xong cần mở lại Loah). macOS 15 trở lên thỉnh thoảng hỏi lại xem Loah có được tiếp tục quay màn hình không: hãy cho phép. Bản ghi không có các cửa sổ của chính Loah, kể cả thanh quay. Ở chế độ *Một cửa sổ*, âm thanh của máy chỉ là âm thanh do app của cửa sổ đó phát: chọn *Toàn màn hình* để ghi mọi âm thanh.
- **Windows:** thanh quay, cửa sổ Cuộc họp và Cài đặt không xuất hiện trong video trên Windows 10 phiên bản 2004 trở lên; các cửa sổ của đọc chính tả (bong bóng và thẻ) thì không được loại ra.
- **Linux:** màn hình đến từ ScreenCast portal của desktop, nên cần `xdg-desktop-portal` và backend của desktop bạn dùng (GNOME, KDE Plasma, Hyprland, hoặc `xdg-desktop-portal-wlr` cho Sway và các desktop wlroots khác), trên cả Wayland lẫn X11, cùng PipeWire (`libpipewire-0.3-0`; desktop có PipeWire thì đã có). Hộp thoại của chính desktop hỏi chia sẻ gì. Âm thanh của máy được ghi bằng `parec`, trong gói `pulseaudio-utils`. Linux không có cách nào loại cửa sổ của Loah khỏi video, nên chúng có thể xuất hiện trong video, cả thanh quay.

### yt-dlp và ffmpeg

**ffmpeg đã đi kèm.** Loah có sẵn một bản build của [FFmpeg](https://ffmpeg.org) theo giấy phép LGPL, tên là `loah-ffmpeg`, nằm cạnh chương trình. Nó đọc các định dạng Loah không tự mở được, mã hoá video của bản ghi và giúp yt-dlp. Bạn không cần cài gì. Cài đặt → *Nhận dạng giọng nói* → *Công cụ cho link* cho thấy Loah đã tìm được gì.

**yt-dlp không đi kèm.** Link cần [yt-dlp](https://github.com/yt-dlp/yt-dlp), bạn tự cài: Loah tìm nó trong đường dẫn tìm kiếm và trong các thư mục thường dùng của trình quản lý gói.

| Hệ thống | Cài yt-dlp |
|---|---|
| macOS | `brew install yt-dlp` |
| Windows | `winget install yt-dlp.yt-dlp` |
| Linux | `pipx install yt-dlp` |

**Dùng ffmpeg của riêng bạn.** LGPL cho phép bạn thay ffmpeg đi kèm Loah. Loah tìm ffmpeg theo thứ tự: chương trình mà biến môi trường `LOAH_FFMPEG` chỉ tới (một đường dẫn, hoặc một tên có trong đường dẫn tìm kiếm; `DICTATE_FFMPEG`, tên mà Dictate từng dùng, vẫn được đọc khi `LOAH_FFMPEG` chưa đặt), rồi `loah-ffmpeg` nằm cạnh chương trình, rồi `ffmpeg` trong đường dẫn tìm kiếm. Giá trị rỗng, hoặc chỉ tới chương trình không chạy được thì bị bỏ qua. App được mở từ desktop không thấy hết những gì terminal của bạn thấy, nên hãy đặt biến ở nơi app sẽ đọc:

- **macOS:** app mở từ Finder hoặc Dock không thấy các biến môi trường của shell. Đặt cho phiên đăng nhập bằng `launchctl setenv`, rồi thoát Loah và mở lại (thiết lập này giữ tới khi bạn khởi động lại Mac), hoặc mở Loah từ terminal với biến đã đặt:

  ```sh
  launchctl setenv LOAH_FFMPEG /path/to/ffmpeg
  ```

- **Windows:** đặt thành biến môi trường của tài khoản bạn (tìm "Edit environment variables for your account" trong menu Start, hoặc dùng `setx` trong terminal), rồi thoát Loah và mở lại:

  ```powershell
  setx LOAH_FFMPEG "C:\path\to\ffmpeg.exe"
  ```

- **Linux:** đặt trong môi trường của phiên desktop (ví dụ một file trong `~/.config/environment.d/` trên desktop dùng systemd, rồi đăng nhập lại), hoặc mở Loah từ terminal với biến đã đặt:

  ```sh
  LOAH_FFMPEG=/path/to/ffmpeg loah
  ```

### Hỏi Claude về cuộc họp (MCP)

Loah có kèm `loah-mcp`, một MCP server chỉ đọc. Nhờ nó, Claude Code và Claude Desktop đọc được các bản ghi trong thư viện của bạn: hỏi "cuộc họp hôm qua chốt gì?" hay "tôi còn việc gì chưa làm?". Claude tự tìm bản ghi, đọc tóm tắt và trích transcript kèm mốc `[mm:ss]`.

| Tool | Làm gì |
|---|---|
| `list_meetings` | Danh sách bản ghi mới nhất trước: ngày, thời lượng, số việc chưa xong, đầu tóm tắt. Lọc được theo ngày hoặc từ khoá |
| `get_meeting` | Tóm tắt đầy đủ của một bản ghi, kèm việc nào đã tick xong trong Loah |
| `get_transcript` | Transcript, lấy được một khoảng thời gian (`start`/`end`); bài dài trả về từng phần |
| `search_meetings` | Tìm trong tiêu đề, tóm tắt và transcript. Không phân biệt hoa thường và dấu: gõ "hop" vẫn ra "họp" |
| `list_action_items` | Việc cần làm của mọi bản ghi, lọc theo người phụ trách, trạng thái (chưa xong / xong) và ngày |

Vị trí của server theo từng hệ thống:

| Hệ thống | Đường dẫn |
|---|---|
| macOS | `/Applications/Loah.app/Contents/MacOS/loah-mcp` |
| Windows | `%LOCALAPPDATA%\Loah\loah-mcp.exe` (thư mục mà bộ cài đã dùng cho tài khoản của bạn; nếu bạn chọn thư mục khác thì dùng thư mục đó) |
| Linux (`.deb`) | `/usr/bin/loah-mcp` |

Chạy nó trong terminal thì nó in ra đúng câu lệnh cho nơi nó đang nằm. **Claude Code:**

```sh
claude mcp add --scope user loah -- /Applications/Loah.app/Contents/MacOS/loah-mcp
```

Trên Windows, trong PowerShell:

```powershell
claude mcp add --scope user loah -- "$env:LOCALAPPDATA\Loah\loah-mcp.exe"
```

Trên Linux, nơi nó nằm trong đường dẫn tìm kiếm:

```sh
claude mcp add --scope user loah -- loah-mcp
```

**Claude Desktop:** Settings → Developer → Edit Config, thêm `loah` vào `mcpServers` trong `claude_desktop_config.json`, rồi khởi động lại Claude:

```json
{
  "mcpServers": {
    "loah": { "command": "/Applications/Loah.app/Contents/MacOS/loah-mcp" }
  }
}
```

Trên Windows, lệnh là đường dẫn ở trên, với mỗi dấu gạch chéo ngược viết hai lần trong JSON:

```json
{
  "mcpServers": {
    "loah": { "command": "C:\\Users\\<you>\\AppData\\Local\\Loah\\loah-mcp.exe" }
  }
}
```

Thư viện nằm trong thư mục Documents, nên trên Mac, lần đầu macOS có thể hỏi cho Claude (hoặc app terminal chạy Claude Code) truy cập thư mục này: chọn Cho phép (Allow). Server chỉ đọc, không sửa hay xóa gì. Biến `LOAH_MEETINGS_DIR` trỏ server sang một thư viện khác (app cũng vậy), và `DICTATE_MEETINGS_DIR` cũng được khi biến kia chưa đặt. Đang dùng MCP server của Dictate? [Nâng cấp từ Dictate](#nâng-cấp-từ-dictate) nói cách trỏ client sang server này mà giữ nguyên tên các tool. Server hiểu cả hai đời giao thức MCP: đời mới (2026-07-28, mỗi request mang phiên bản trong `_meta`) và bắt tay `initialize` của 2025-11-25 trở về trước.

## Cài đặt

Mở Cài đặt bằng icon trên khay → *Cài đặt…*.

- **Chung:** ngôn ngữ giao diện, tự mở khi khởi động máy, ngôn ngữ đọc, phím tắt, chế độ bấm hoặc giữ, tự viết vào ô chữ đang chọn, lưu đoạn đọc, các quyền (trên Mac), phím tắt quay, thư mục cuộc họp, và liên kết tới trang giấy phép.
- **Nhận dạng giọng nói:** engine cho đọc chính tả và cho file (Whisper trên máy hoặc API transcript), ngôn ngữ transcript, tách người nói, các model Whisper và các công cụ cho link.
- **AI sửa & tóm tắt:** kiểu sửa mặc định, tự sửa khi nói xong, model tóm tắt và từ vựng riêng của bạn.
- **Tài khoản & API key:** nhà cung cấp AI và dịch vụ transcript, mục nào cũng có nút **Kiểm tra**.

Loah lưu dữ liệu ở đâu:

| | macOS | Windows | Linux |
|---|---|---|---|
| Cài đặt | `~/Library/Application Support/Loah/settings.json` | `%APPDATA%\Loah\settings.json` | `~/.config/Loah/settings.json` |
| Model Whisper, model tách người nói, đoạn đọc đã lưu | `~/Library/Application Support/Loah/` | `%LOCALAPPDATA%\Loah\` | `~/.local/share/Loah/` |
| Thư viện cuộc họp | `~/Documents/Loah Meetings` | `Documents\Loah Meetings` | `~/Documents/Loah Meetings` |
| API key | Keychain | Credential Manager | Secret Service |

Thư viện do Dictate tạo vẫn nằm ở `Dictate Meetings` và được dùng tại đó; xem [Nâng cấp từ Dictate](#nâng-cấp-từ-dictate).

## Quyền riêng tư

Loah không có tài khoản, không thu thập số liệu sử dụng hay telemetry. App chỉ kết nối tới các dịch vụ bạn tự cài đặt, tới trang của link bạn thêm (qua yt-dlp), tới Hugging Face và GitHub để tải model (các model Whisper và hai model tách người nói) và, trên Windows 10 chưa có WebView2, tới Microsoft khi bộ cài tải nó về. Hình và tiếng của bản ghi được phát lại bởi một server nhỏ nằm trong app, chỉ lắng nghe trên `127.0.0.1`, với một token ngẫu nhiên trong địa chỉ.

- **Nằm trên máy bạn:** bản ghi, transcript và tóm tắt (trong các thư mục ở trên), và Whisper, thứ nhận dạng giọng nói ngay trên máy. API key nằm trong kho credential của hệ thống.
- **Gửi cho nhà cung cấp AI** (OpenRouter, trừ khi bạn chọn dịch vụ khác): đoạn chữ bạn đọc khi được sửa hay viết lại, và transcript của bản ghi khi tóm tắt hoặc khi bạn hỏi về nó. Dùng endpoint chạy trên máy như Ollama thì phần này cũng nằm trên máy.
- **Gửi cho API transcript,** chỉ khi bạn chọn API làm engine: âm thanh của đoạn đọc hoặc của bản ghi.
- **Claude đọc** qua MCP server: nội dung bản ghi, khi Claude đọc nó.

## Phần mềm của bên thứ ba

Loah được xây trên công sức của nhiều người: whisper.cpp và các model Whisper, sherpa-onnx và ONNX Runtime, các model tách người nói của pyannote và NeMo, Tauri, Svelte cùng nhiều thư viện Rust. Bộ cài còn có [FFmpeg](https://ffmpeg.org), theo giấy phép GNU LGPL phiên bản 2.1 trở lên. [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) (tiếng Anh) liệt kê tất cả cùng giấy phép của chúng, có lời cam kết bằng văn bản về mã nguồn FFmpeg, và nói cách FFmpeg đi kèm Loah được build ra sao và thay nó thế nào. Mỗi bản phát hành đều đính kèm mã nguồn FFmpeg và các dòng cấu hình đã dùng để build. Cài đặt → Chung có liên kết **Giấy phép** tới chính file này.

Các file giấy phép đi kèm mọi bộ cài: `LICENSE`, `THIRD_PARTY_NOTICES.md`, `THIRD_PARTY_LICENSES.txt` (toàn văn giấy phép và dòng bản quyền của mọi thứ trong Loah, kể cả từng crate Rust và package npm) và `COPYING.LGPLv2.1` (văn bản LGPL cho FFmpeg). Trên macOS chúng nằm trong `Loah.app/Contents/Resources/Licenses` (trong Finder, bấm chuột phải vào app rồi chọn *Show Package Contents*); trên Windows, trong thư mục `Licenses` cạnh `loah.exe` (`%LOCALAPPDATA%\Loah\Licenses`); trên Linux, trong `/usr/share/doc/loah`.

## Đóng góp

Báo lỗi, góp ý và pull request đều được hoan nghênh: mở một [issue](https://github.com/henryonai/loah/issues/new/choose). [CONTRIBUTING.md](CONTRIBUTING.md) (tiếng Anh) hướng dẫn build, chạy test, dịch giao diện và viết commit, còn [app/README.md](app/README.md) là hướng dẫn cho người phát triển app. Mọi người tham gia đều theo [Quy tắc ứng xử](CODE_OF_CONDUCT.md). Lỗi bảo mật xin báo riêng như trong [SECURITY.md](SECURITY.md), đừng mở issue công khai.

## Giấy phép

[MIT](LICENSE) © 2026 Henry Hoang
