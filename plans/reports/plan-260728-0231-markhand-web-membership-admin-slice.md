# Mini-milestone — Membership + Admin API slice (mở khoá P2-11/P2-12)

Date: 2026-07-28
Base commit: `97a29dd` (master, sau PR #316)
Nguồn: audit Phase 1C 2026-07-27 (`plans/markhand-web/backlog/phase-1c/issues/README.md`).

## 1. Mục tiêu và ranh giới

**Mục tiêu:** xây lát mỏng nhất của Phase 1C đủ để **gỡ chặn P2-11 (member/role admin)
và P2-12 (usage/quota)** trong web — không làm cả Phase 1C.

Cắt từ hai issue:
- **1C-02** (hiện **Backlog**) — membership + invite + last-owner invariant.
- **1C-11** (hiện **In progress**, nửa admin = 0 endpoint) — chỉ phần **admin/audit read
  API** cho member/role/usage. KHÔNG làm audit retention, cloud/data-export event.

**Nằm NGOÀI lát này** (giữ nguyên trạng thái phase): 1C-01 org create/switch, ACL
cache/version (1C-05), per-org fairness (1C-10), denial suite gắn kết (1C-12), security
gate (1C-13). Xem mục 9.

## 2. Ba cảnh báo phải đọc trước — không được bỏ qua

### C1. Lát này **nhảy qua phase gate** — phải bù bằng test, không bằng niềm tin
Phase 1C theo thiết kế chỉ activate sau khi **Phase 1B gate đóng** (R04/R06 còn pending),
và **1C-12 denial suite chưa tồn tại**. Ta đang thêm một **surface mutation theo tenant**
(mời/xóa/đổi-role thành viên) và một **surface tạo membership** (accept invite = cấp quyền
truy cập tenant) *trước khi* có suite denial cross-org gắn kết.

→ Hệ quả bắt buộc: **mọi endpoint mới trong lát này phải kèm test denial cross-org DB-gated
của riêng nó** (org khác không thấy/không sửa được member của org này; invite của org A
không accept được vào org B). Đây chính là "kéo sớm" đúng phần 1C-12 tương ứng với surface
mới. Không được để "mini-milestone" thành "bỏ test bảo mật".

### C2. Hệ thống vẫn **single POC org** — đây là "quản thành viên của org đang có"
Seed hiện gắn cứng một org (`11111111-…-1111`, `migrations/0011:11`); **không có đường
org-create** (1C-01). Nên lát này là **quản lý thành viên của org hiện hữu**, không phải
provision org mới. Invite-accept cho một user MỚI vào org đang có thì chạy được; tạo org
thứ hai thì không. Web P2-11 chỉ cần cái trước — nhưng phải nói rõ giới hạn này trong doc
API để không ai tưởng đã multi-org.

### C3. Invite **không có email** (out of scope 1C-02)
Token plaintext hiển thị **đúng một lần** cho admin qua response, copy ra kênh ngoài. Token
chỉ lưu DB dạng **hash** (`org_invites.token_hash`, đã có). Plaintext **không được** log/
lưu/vào audit — `FORBIDDEN_METADATA_KEYS` trong `services/audit.rs` đã chặn key kiểu token,
nhưng code mới vẫn phải tự cẩn thận đường đi của plaintext.

## 3. Điều kiện vào — cái đã có (đo được, không đoán)

| Thứ | Trạng thái | Bằng chứng |
|---|---|---|
| Bảng `org_memberships` | có — `(org_id,user_id,role∈{owner,admin,editor,viewer},created_at)`, PK `(org_id,user_id)`; **chưa có** cột state/version | `migrations/0001:26` |
| Bảng `org_invites` | có — hashed single-use: `token_hash UNIQUE ≥32`, `expires_at`, `accepted_at`/`revoked_at` + `ck_…terminal_xor`; **không** cột plaintext | `migrations/0003:93` |
| Permission `member.manage` | seed rồi nhưng **chưa dùng** ở đâu | `migrations/0011:44` |
| Resolver membership | có — `resolve_org_context` deny khi thiếu membership/disabled; JWT chỉ là hint | `auth/permissions.rs:39`, `auth/middleware.rs:144` |
| Guard deny-by-default | có — `require_permission` áp ở route+service | `auth/permissions.rs:118` |
| Audit append-only + writer + redaction | có, có test | `migrations/0026`, `services/audit.rs:447` |
| Hàm đọc audit `list_recent` | **có nhưng chưa gọi**, chưa phân trang/filter | `db/audit.rs:11` |
| Quota read (cho P2-12) | có — `quota_limit`/`committed_usage`/`active_reserved`/`usage` | `db/quota.rs:61/106/147/168` |
| Session/refresh family revoke | có | `auth/session.rs:862` |

Nghĩa là **schema + resolver + guard + audit + quota-read đã sẵn**; phần thiếu chủ yếu là
**service logic + endpoint + last-owner invariant + audit action + test**.

## 4. Surface API tối thiểu

OpenAPI-relative path (không prefix `/api/v1`). "Guard" = permission qua `require_permission`.

| # | Method + Path | Việc | Guard | Ghi chú |
|---|---|---|---|---|
| E1 | `GET /members` | List thành viên org hiện tại (user, role, joined_at) | `member.manage` | read; không lộ user org khác |
| E2 | `GET /members/invites` | List invite đang mở (email, role, expires, trạng thái) | `member.manage` | không trả token_hash |
| E3 | `POST /members/invites` | Tạo invite single-use; trả **plaintext token 1 lần** | `member.manage` | owner mới mời role owner; audit `MemberInvite` |
| E4 | `POST /members/invites/{inviteId}/revoke` | Thu hồi invite chưa dùng | `member.manage` | set `revoked_at`; audit `MemberInviteRevoke` |
| E5 | `POST /members/invites/accept` | Accept bằng token → tạo membership | **auth-only** (user đã login, chưa cần `member.manage`) | verify hash+expiry+chưa terminal; audit `MemberInviteAccept` |
| E6 | `PATCH /members/{userId}` | Đổi role thành viên | `member.manage` | **last-owner invariant**; audit `MemberRoleChange` |
| E7 | `DELETE /members/{userId}` | Xóa thành viên khỏi org | `member.manage` | **last-owner invariant** + revoke family; audit `MemberRemove` |
| E8 | `GET /usage` | Tổng hợp usage/limit/reserved mỗi ResourceKind | `member.manage` hoặc quyền đọc org | read; ghép từ `db/quota.rs` sẵn có |

> **Last-owner invariant (E6, E7):** trong **một transaction**, lock các dòng owner của org
> (`SELECT … FOR UPDATE`), đếm owner còn lại; nếu thao tác làm số owner active về 0 → **409
> `last_owner`** (không cho remove/downgrade owner cuối). Đây là điểm 1C-02 mà audit ghi
> "grep rỗng" — phải viết mới, và là chỗ dễ sai nhất (race hai admin cùng gỡ hai owner).

> **Enforce tức thời khi xóa/hạ quyền:** resolver chạy **live mỗi request**, nên user bị gỡ
> sẽ `MembershipMissing` ở request kế (`auth/permissions.rs`). SSE re-resolve mỗi pull nên
> stream đang chạy tự đóng. Access token ngắn hạn vẫn hợp lệ tới hết hạn → E7 (và E6 hạ
> quyền) **phải revoke refresh-token family của user trong org** (`auth/session.rs:934`
> `revoke_all_user_families`) để cắt cứng, không chờ token hết hạn.

## 5. Nghĩa vụ đi kèm MỖI endpoint mới (đừng lặp lại lỗ cũ)

1. **Parity ba nơi phải khớp** — thêm route vào `routes/*.rs`, vào `ROUTE_INVENTORY`
   (`api/openapi.rs:14`), và vào `openapi.yaml` **kèm requestBody + response schema** (parity
   giờ kiểm mức schema — `openapi_schema_completeness_gaps`). Thiếu một chỗ → CI đỏ.
2. **Audit mọi mutation** — E3/E4/E5/E6/E7 phải `record_in_txn` với actor/org/action/target/
   result/request-id. Cần **thêm 5 biến thể vào `enum AuditAction`** (`services/audit.rs`):
   `MemberInvite, MemberInviteAccept, MemberInviteRevoke, MemberRoleChange, MemberRemove`, và
   khai `metadata_keys` allowlist cho từng cái (KHÔNG cho free-text, KHÔNG token).
3. **RLS/tenant** — mọi query đi qua `with_org_txn` (GUC `app.org_id`), không tự nối org_id
   tay. Member của org khác phải vô hình (RLS + predicate), không phải 403-lộ-tồn-tại.
4. **Sinh lại contract TS** — sau khi sửa `openapi.yaml`: `pnpm --dir web api:generate`;
   `pnpm api:check` chặn drift. Web P2-11/P2-12 sẽ có type ngay.

## 6. Migrations cần thêm

- **M1** — cột membership version (tùy chọn nhưng nên): `ALTER TABLE org_memberships ADD
  COLUMN version bigint NOT NULL DEFAULT 1` + bump khi role đổi. **Chưa xây cache
  invalidation trên nó** (1C-05 cache chưa có) — chỉ thêm cột để về sau forward-compatible;
  ghi rõ trong migration là "reserved for 1C-05 cache". Nếu muốn tối giản, có thể hoãn M1.
- **M2** — nếu cần cột `state` cho membership (active/suspended) phục vụ "suspend" của P2-11:
  `ADD COLUMN state text NOT NULL DEFAULT 'active' CHECK (state IN ('active','suspended'))`.
  Suspend = state suspended (resolver coi như thiếu membership). **Quyết định:** P2-11 có nút
  "suspend" — nếu muốn suspend khác remove thì cần M2; nếu MVP chỉ remove thì bỏ M2.
- Theo convention repo: migration **expand-only**, có trong `manifest.json`, test
  `schema_migrations.rs` phải xanh (idempotent + exact columns).

## 7. Test bắt buộc

- **Unit (fast):** last-owner invariant (đủ owner → cho, còn 1 owner → 409); invite hash
  verify + expiry + terminal reuse reject; audit allowlist cho 5 action mới.
- **DB-gated (`#[ignore]`, chạy job `rust-integration`):**
  - concurrent last-owner: hai admin cùng remove hai owner → đúng một thành công, org còn ≥1 owner.
  - invite replay/expiry: token đã accept/revoke/hết hạn → reject; accept đúng → membership tạo + audit.
  - **cross-org denial (bù C1):** admin org A không list/patch/delete member org B; invite org A
    không accept vào org B; usage org A không lộ số org B. **Đây là điều kiện đóng lát này.**
  - revoke-on-remove: sau E7, refresh family của user bị vô hiệu; SSE đang chạy đóng.
- **Contract:** `pnpm api:check` xanh; ROUTE_INVENTORY parity xanh.

## 8. Thứ tự làm (work items)

```
W1  Migration M2 (+M1 nếu chọn) + models + repo (org_memberships/org_invites CRUD qua with_org_txn)
W2  AuditAction +5 biến thể + metadata_keys allowlist  (chặn: audit mọi mutation ở W4/W5)
W3  Service: invite create/accept/revoke (hash, expiry, terminal)  +  last-owner invariant (txn, row-lock)
W4  Routes E1–E5 (list/invite/accept/revoke) + guard + audit + openapi.yaml + ROUTE_INVENTORY
W5  Routes E6–E7 (role change/remove) + last-owner + revoke family + audit + parity
W6  Route E8 /usage (ghép db/quota.rs) + openapi + parity
W7  Test: unit + DB-gated (last-owner concurrent, invite replay, cross-org denial, revoke-on-remove)
W8  pnpm api:generate → contract TS; xác nhận web P2-11/P2-12 có type; api:check xanh
```

W1–W3 là nền (không có endpoint public); W4–W6 mở surface; W7 là điều kiện đóng (C1); W8 giao
sang web. W2 chặn W4/W5 (không audit thì không merge được mutation).

Có thể spawn subagent: một agent làm W1–W3 (service/repo/migration, không public surface),
một agent làm W4–W6 (routes+parity) sau khi W2 xong; W7 nên do người điều phối kiểm vì đây là
phần bảo mật (kéo sớm 1C-12). Opus review last-owner + cross-org denial.

## 9. Không thuộc kế hoạch này

Org create/list/switch (1C-01), ACL cache/version + groups grant (1C-05), per-org fairness/GPU
scheduler (1C-10), audit retention + cloud/data-export event (phần còn lại 1C-11), denial suite
gắn kết đầy đủ mọi surface (1C-12), security/load gate + report (1C-13), email invite/SCIM/MFA.

Đóng lát này **không** đóng Phase 1C và **không** đạt exit gate 1C — nó chỉ chuyển 1C-02 từ
Backlog sang In-progress-thực-chất và bồi phần admin của 1C-11, đủ để P2-11/P2-12 rời khỏi
trạng thái Blocked.
