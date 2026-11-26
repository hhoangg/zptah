# Chrome Web Store listing: zptah Browser

Source of truth for the dashboard fields (spec 13.1). Paste these into the Developer Dashboard by
hand; nothing here is uploaded by CI. Change this file in the same PR as any change to the
manifest, the privacy page or the behavior it describes.

## Fields

| Field | Value |
|---|---|
| Name | zptah Browser |
| Category | Developer Tools (fallback: Productivity) |
| Default language | English |
| Additional language | Vietnamese (Tiếng Việt) |
| Visibility | Public |
| Privacy policy URL | https://zptah.app/privacy |
| Homepage URL | https://zptah.app |
| Support URL | https://github.com/hhoangg/zptah/issues |
| Publisher | the maintainer's verified publisher account (2-step verification on) |

## Graphic assets

Generated from the zptah app logo by `scripts/gen-icons.py` (Pillow; the build only copies the
committed `icons/`):

- Store icon 128x128: `store/icon-128.png`
- Small promo tile 440x280: `store/promo-tile-440x280.png`
- Toolbar and manifest icons (16, 32, 48, 128): `icons/`

## Single purpose statement

Lets the zptah desktop app's assistants (Monos) read and act in your browser tabs when you ask
them to, under the approvals you set in zptah. It talks only to the zptah app on your computer.

## Short description (at most 132 characters)

English (112):

> Let your zptah Monos read and use your tabs, with clear approvals for payments, deletions and other risky steps.

Vietnamese (120):

> Cho các Mono của zptah đọc và dùng tab của bạn, có xác nhận rõ ràng cho thanh toán, xóa dữ liệu và các bước rủi ro khác.

## Long description, English

zptah Browser connects the zptah desktop app to your browser, so the assistants you set up in zptah
(called Monos) can read and work in your tabs when you ask them to.

What it does
- A Mono can list your tabs, read a page, find things on it, take a screenshot, and click, type,
  select and scroll when a task needs it.
- Each Mono works in its own named, colored tab group, so you always see which tabs it uses.
  A glow and a Stop button appear on the tab it is working in.
- Risky steps need your approval in the zptah app first: payments, deleting things, changing
  account security, sharing and permissions, posting publicly, accepting terms, entering codes,
  uploading files. Passwords are never typed by a Mono.
- You choose, per Mono, whether it may only read or also act, and which sites it may use.
- One click on the toolbar icon stops every Mono at once.

How it stays private
- The extension talks only to the zptah app on your computer, through Chrome's native messaging.
  It has no server of its own and makes no network requests.
- No analytics, no telemetry, no crash reports, no ads, and nothing is sold or transferred.
- Page content a Mono reads becomes part of that Mono's conversation in zptah, which zptah sends to
  the AI provider you configured, exactly as for everything else in that conversation.
- The activity log stays in the zptah app on your computer.

Requirements
- The zptah desktop app (free, open source) installed and paired with this extension. The extension
  does nothing on its own. Get it at https://zptah.app.
- Chrome 120 or newer. Not available in Incognito windows.

Debugger permission
- Chrome shows a warning about "debugging this browser" when you install, because it does not allow
  this permission to be optional. The extension does not use it unless you turn on "Use the debugger
  fallback" in its options (off by default), for sites that ignore simulated input, rich text editors
  and page dialogs. Chrome shows its debugging bar while it is attached, and it is released when the
  work ends or you press Stop.

Privacy policy: https://zptah.app/privacy
Source code: https://github.com/hhoangg/zptah

## Long description, Vietnamese

zptah Browser kết nối ứng dụng zptah trên máy tính với trình duyệt, để các trợ lý bạn tạo trong
zptah (gọi là Mono) có thể đọc và làm việc trong các tab của bạn khi bạn yêu cầu.

Tiện ích làm gì
- Mono có thể liệt kê tab, đọc trang, tìm nội dung trên trang, chụp ảnh màn hình, và nhấp, gõ, chọn,
  cuộn khi nhiệm vụ cần.
- Mỗi Mono làm việc trong nhóm tab riêng có tên và màu, nên bạn luôn thấy nó đang dùng tab nào.
  Tab đang được dùng có viền sáng và nút Dừng.
- Các bước rủi ro cần bạn đồng ý trong ứng dụng zptah trước: thanh toán, xóa dữ liệu, đổi bảo mật tài
  khoản, chia sẻ và phân quyền, đăng công khai, chấp nhận điều khoản, nhập mã, tải tệp lên. Mono không
  bao giờ gõ mật khẩu.
- Bạn chọn cho từng Mono: chỉ được đọc hay được thao tác, và được dùng những trang nào.
- Một cú nhấp vào biểu tượng trên thanh công cụ dừng tất cả Mono ngay.

Quyền riêng tư
- Tiện ích chỉ nói chuyện với ứng dụng zptah trên máy bạn, qua native messaging của Chrome. Nó không có
  máy chủ riêng và không tự gửi yêu cầu mạng.
- Không phân tích số liệu, không đo đạc từ xa, không báo cáo lỗi, không quảng cáo, không bán hay
  chuyển giao dữ liệu.
- Nội dung trang mà Mono đọc trở thành một phần của cuộc trò chuyện của Mono đó trong zptah, và zptah
  gửi nó tới nhà cung cấp AI bạn đã cấu hình, giống như mọi nội dung khác của cuộc trò chuyện.
- Nhật ký hoạt động nằm trong ứng dụng zptah trên máy của bạn.

Yêu cầu
- Ứng dụng zptah (miễn phí, mã nguồn mở) đã cài và ghép đôi với tiện ích. Tiện ích không tự làm gì.
  Tải tại https://zptah.app.
- Chrome 120 trở lên. Không dùng được trong cửa sổ ẩn danh.

Quyền debugger
- Chrome hiện cảnh báo "gỡ lỗi trình duyệt này" khi cài đặt, vì Chrome không cho quyền này là tùy chọn.
  Tiện ích không dùng nó trừ khi bạn bật "Use the debugger fallback" trong trang cài đặt (mặc định tắt),
  dùng cho các trang bỏ qua thao tác giả lập, trình soạn thảo văn bản phong phú và hộp thoại của trang.
  Chrome hiện thanh gỡ lỗi khi quyền này đang dùng, và nó được thả khi việc xong hoặc khi bạn bấm Dừng.

Chính sách quyền riêng tư: https://zptah.app/privacy
Mã nguồn: https://github.com/hhoangg/zptah

## Store data-usage form (spec 13.3, decision 6: conservative)

Declare as handled: **Website content** and **Web history** (tab addresses). Reason: the content
reaches the user's AI provider through the app.

Not handled: personally identifiable information, health, financial and payment information,
authentication information, personal communications, location, user activity (keystrokes, clicks
and mouse position are not recorded by the extension; it performs actions the user's assistant
asks for).

Certify all three: no sale or transfer to third parties outside the approved use cases, no use or
transfer for purposes unrelated to the single purpose, no use for creditworthiness or lending.
Limited Use: the data is used only to provide the single purpose.

Remote code: No. The extension loads no remote code (MV3; the build scan forbids eval, remote
imports and network APIs).

## Test instructions for reviewers

The extension only works with the zptah desktop app (macOS, Linux, Windows) installed from
https://github.com/hhoangg/zptah/releases and paired through the extension's Connect page. Without
the app the extension shows "zptah is not running" and does nothing. Provide a short screen
recording of pairing and one read and one approved action in the privacy-safe demo profile.
