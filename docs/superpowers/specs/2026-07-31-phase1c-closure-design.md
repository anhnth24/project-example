# Thiết kế đóng Phase 1C theo luồng implement/review

Ngày: 2026-07-31  
Trạng thái: Đã sửa theo review vòng 2; chờ owner duyệt tài liệu
Phạm vi: Phase 1C — multi-org security, denial suite và security/load gate

## 1. Mục tiêu

Đóng Phase 1C bằng bằng chứng thực thi, không chỉ đổi trạng thái issue. Kết quả cuối
cùng phải chứng minh:

- RBAC/ACL nhất quán ở route, service, PostgreSQL, Qdrant, object storage và worker;
- không rò nội dung, metadata, sự tồn tại hoặc trạng thái giữa các org;
- revoke, cache invalidation, quota recovery và worker fairness có bound kiểm chứng;
- suite chạy trong GitHub CI và gate chạy trên một deployment multi-org thật;
- catalog, report và GitHub issue chỉ chuyển `Done` sau khi bằng chứng tương ứng xanh.

Phase 1B tại `master` commit `9ddfc06` là baseline đã đóng. Công việc này không mở lại
Phase 1B nếu không phát hiện regression trực tiếp.

## 2. Nguyên tắc phạm vi

### 2.1 Nguồn sự thật

- Acceptance gốc nằm trong
  `plans/markhand-web/phase-1c-multi-org-security.md`.
- Trạng thái và bằng chứng nằm trong
  `plans/markhand-web/backlog/phase-1c/issues/README.md`.
- Hành vi có thể kiểm chứng của code, migration và test được ưu tiên hơn mô tả audit
  cũ. Claim stale phải được sửa trong chính PR xác minh nó.
- Một issue chỉ `Done` khi tất cả acceptance còn áp dụng có test xanh ở đúng môi
  trường. “Code đã landed”, test soft-skip hoặc CI path-filter skip không phải bằng
  chứng hoàn thành.

### 2.2 Surface chưa tồn tại

Không tạo API giả chỉ để tick checklist. `export`, `autocomplete`, signed URL, PII và
intelligence chưa có surface runtime trong Phase 1C:

- denial matrix ghi `N/A` kèm bằng chứng source-scan và issue/phase sẽ sở hữu surface;
- signed URL được thay bằng capability token, nên suite kiểm capability
  tamper/expiry/replay/IDOR;
- permission dự kiến nhưng chưa có operation được khai báo reserved, không tự cấp
  quyền hoặc tạo endpoint rỗng;
- embedding-token metering là `N/A` chỉ khi qualifying environment dùng local/mock
  provider không phát sinh token billable. Cloud/shared embedding provider không được
  bật trong qualifying environment cho tới khi job lifecycle meter được token usage;
  `concurrent_jobs` tiếp tục bound compute trong cấu hình local/mock.

Khi một surface được bổ sung về sau, guard inventory và denial manifest phải buộc PR
đó thêm test tương ứng. Validator đối chiếu denial manifest với guard/route inventory:
operation mới không có row executable hoặc `N/A` hợp lệ làm CI fail; row `N/A` cũng
fail khi operation tương ứng xuất hiện.

### 2.3 Không thuộc phạm vi

- Custom-role builder, OIDC/group sync, billing và SIEM archive;
- embedding model hoặc LLM provider mới;
- thay đổi business feature Phase 2/3 không cần cho security gate;
- tuyên bố production-scale nếu environment không khớp profile đã phê duyệt.

### 2.4 Accepted risk: audit retention

Phase plan P1C.6 yêu cầu audit retention configurable, trong khi catalog 1C-11 đã
defer retention/TTL và Phase 4 sở hữu audit tamper evidence, retention và export.
Thiết kế này chọn disposition tường minh:

- Phase 1C không thêm purge/TTL cho append-only audit log;
- PR 1 phải sửa P1C.6/catalog cho nhất quán và ghi accepted risk
  `AR-1C-AUDIT-RETENTION` trong risk register, approver là security owner và operations
  owner;
- risk chỉ được chấp nhận cho POC/non-production multi-org, hết hiệu lực trước lần
  production multi-org đầu tiên hoặc Phase 4 gate, điều kiện nào đến trước;
- qualifying report phải ghi audit row growth và xác nhận environment không phải
  production; không được dùng Phase 1C pass để tuyên bố audit lifecycle production-ready.

Nếu owner không duyệt accepted risk này khi review spec, retention config + purge job
phải được đưa vào PR 3 và 1C-11 giữ `In progress` cho tới khi test lifecycle xanh.

## 3. Quyết định kiến trúc

### 3.1 Canonical RBAC contract

Tạo fixture JSON
`crates/server/openapi/builtin-role-catalog.json` làm canonical contract cho built-in
role và permission. Fixture chứa:

- stable permission key;
- trạng thái `active` hoặc `reserved`;
- role `owner/admin/editor/viewer`;
- grant matrix cho permission active;
- `requiredCollectionAccess` khi operation collection-scoped;
- restriction như “admin không quản owner”;
- metadata `conditionalPolicy` để mô tả policy tương lai nhưng không tự tạo grant.

Database vẫn là authority runtime. Test DB-gated đối chiếu catalog đã provision với
fixture; OpenAPI test và web role presentation sinh hoặc đọc cùng fixture thay vì
chép một matrix thứ hai. Historical migration không được sửa; nếu matrix runtime đổi,
thêm migration expand-only và cập nhật fixture trong cùng PR.

Permission chỉ được `active` khi fixture validator chỉ ra operation runtime thật.
Active permission được seed vào bảng `permissions`; fixture cho phép active permission
có zero default grants. Permission chưa có operation giữ `reserved`, không có row
runtime trong `permissions` và không được seed grant cho tới migration kích hoạt nó.
Guard enforcement của operation active thuộc 1C-04/PR 3, không phải điều kiện seed
catalog của 1C-03.

PR 1 phải disposition rõ các lệch hiện tại thay vì sửa plan cho khớp code một cách
ngầm:

- `doc.publish`, `jobs.system` và `doc.quarantine.review` được bổ sung vào phase-plan
  matrix dưới dạng active; `doc.quarantine.review` giữ zero default grants có chủ đích;
- `settings.manage`, `intel.use`, `pii.manage`, `export.run` giữ reserved và ungranted
  vì chưa có operation runtime;
- built-in editor không có `doc.delete`. Policy “own/explicit” được defer tới policy/
  custom-role work; fixture giữ `conditionalPolicy` chỉ để ghi provenance, không phải
  grant runtime;
- conditional policy như “viewer theo org policy” trên permission reserved là
  metadata không normative và không được resolver coi là quyền.

PR 1 đóng 1C-03 sau khi fixture, DB matrix, OpenAPI/web consumer và operation-reference
validator cùng xanh trong `rust-integration`. PR 3 đóng 1C-04 bằng guard inventory;
inventory phải derive `requiredCollectionAccess` từ canonical fixture qua mapping
operation → permission, không khai báo access level lần thứ hai.

### 3.2 ACL semantics

ACL áp dụng cho active membership:

- `private`: collection owner hoặc explicit user grant; group/role grant không mở
  private collection;
- `org`: member active có base permission tương ứng;
- `groups`: collection owner, explicit user grant, membership trong group được grant,
  hoặc role hiện tại được grant qua `collection_role_access`.

Mọi explicit grant phải đạt `access_level` tối thiểu của operation. Guard inventory
phân loại operation collection-scoped theo thứ tự:

- `read`: list/search/Q&A/citation/preview/download/status/SSE;
- `write`: toàn bộ `read` cộng upload/publish/reindex;
- `admin`: toàn bộ `write` cộng delete và ACL mutation.

Operation system-only như reconcile/maintenance không suy quyền từ collection grant;
chúng dùng service identity riêng.

Semantic reference là hàm logic
`allowed(user, collection, permission, required_access)`: user và membership active,
role có base permission, collection thuộc đúng org, visibility cho phép principal và
explicit grant (nếu cần) đạt access level. Resolver trong `auth/permissions.rs` là
phép chiếu collection của hàm này; PostgreSQL `acl_predicate_sql`, Qdrant
allowed-collection filter, citation hydration, download và jobs phải thực thi cùng
predicate.

Allow-list trong `OrgContext` được định nghĩa là phép chiếu tại
`permission = qa.query`, `required_access = read`; principal không có `qa.query` nhận
allow-list rỗng. Write/admin không suy ra từ allow-list này: service guard và shared SQL
predicate phải re-check permission cùng required access của operation. Service identity
system-only nhận scope tường minh theo §3.3, không đi qua phép chiếu `qa.query`.

Equivalence test PR 2 dùng cùng fixture nhiều trạng thái và so tập collection resolver
trả về với tập row SQL predicate trả về cho phép chiếu trên; test riêng pin
`write/admin` không được thỏa bởi grant hẹp hơn. Không adapter nào được mở rộng scope
khi timeout, payload malformed hoặc dependency lỗi.

Không cho dormant group/role grant trên collection `private` hoặc `org`:

- insert/update `collection_group_access` hoặc `collection_role_access` chỉ hợp lệ khi
  collection đang có visibility `groups`;
- grant trigger phải lock row collection cha bằng `FOR NO KEY UPDATE` (hoặc cơ chế
  deferred tương đương) trước khi kiểm visibility; collection visibility update giữ
  cùng row lock để insert-grant và visibility-flip song song không thể cùng commit
  thành trạng thái dormant dưới `READ COMMITTED`;
- đổi visibility khỏi `groups` phải xóa group/role grant trong cùng transaction trước
  khi đổi; DB invariant từ chối trạng thái còn grant;
- `acl_mutate::revoke_collection_access_for_principal` phải xóa group/role grants
  trước khi set `private`, rồi mới xử lý direct-user grant; containment test phải dùng
  groups collection có cả group và role grant;
- migration preflight phải phát hiện row group/role grant hiện hữu trên collection
  không phải `groups` và fail với diagnostic collection IDs; không tự kích hoạt hoặc
  silently delete grant. Fixture migration hiện hữu phải đổi collection sang `groups`
  trước khi seed grant;
- test pin private collection không mở qua group/role và visibility flip không âm thầm
  kích hoạt grant cũ; test hai transaction pin race grant-vs-flip;
- PR 2 sửa wording P1C.3 để ghi rõ private chỉ nhận direct-user grant và groups nhận
  user/group/role grant.

Mọi mutation ảnh hưởng scope phải bump `orgs.acl_version` trong cùng transaction.
Trigger phải phủ user, group, role grants, group membership, role permission,
collection visibility và membership state. Cache hit tiếp tục freshness-check với
PostgreSQL; lỗi freshness không được tin cache cũ.

### 3.3 Dual-layer authorization

Mỗi operation business hiện hữu có:

1. route guard để trả HTTP contract đúng;
2. service guard để direct call, worker hoặc call-site tương lai không bypass route.

Guard inventory là artifact machine-checked ánh xạ operation → permission → route →
service entry point. Test fail nếu thêm operation mà không đăng ký, nếu operation
không có đủ hai guard, hoặc nếu inventory trỏ tới guard không tồn tại.

Worker, reconcile và maintenance dùng service identity có permission tối thiểu, tenant
scope rõ ràng và không có cờ `internal=true` để bypass. Dedicated PostgreSQL role
`markhand_worker` phải được provision trong POC deployment; fallback app-role chỉ giữ
tương thích dev và không được dùng làm evidence cho gate.

### 3.4 Unified denial suite

Tạo shared fixture:

- 2 org;
- ít nhất 3 user mỗi org, gồm owner/admin/member có quyền hạn khác nhau;
- collection `private`, `org`, `groups`;
- document và collection trùng tên giữa hai org;
- token phát hành trước downgrade/suspend/remove;
- indexed content có marker riêng cho từng org.

Tạo denial manifest ánh xạ mỗi surface tới test executable hoặc trạng thái `N/A` có
lý do. Suite tái sử dụng helper hiện có, nhưng có một test binary/module rõ ràng để CI
có thể chạy và báo cáo riêng. Assertion âm phải kiểm cả status/error code lẫn việc
response không chứa marker, ID, tên, object key hoặc metadata của org khác.

Manifest validator phải join với guard inventory và `ROUTE_INVENTORY`. Mọi business
operation/route phải có denial row; test reference phải resolve tới test đã đăng ký.
`N/A` chỉ hợp lệ khi source-scan chứng minh surface chưa tồn tại hoặc contract ghi rõ
capability thay thế.

Suite phủ HTTP và direct-service/repository cho:

- list/search/FTS/vector/Q&A/citation;
- preview/download capability;
- delete/reindex/job/SSE;
- audit, worker, reconcile và cache sau org switch;
- stale token và in-flight Q&A sau revoke;
- RLS, pool contamination và privileged worker misuse.

### 3.5 Security/load gate

Thêm gate `G1C-*` vào registry với `failureDisposition: block-phase-1c`. Gate tối thiểu
gồm:

- zero cross-tenant leakage;
- membership/ACL revoke bound;
- quota recovery sau crash/retry/cancel/timeout;
- noisy-neighbor fairness/latency;
- dependency và container vulnerability policy;
- audit coverage của administrative mutation hiện hữu.

Mỗi gate có environment ID, command, metric, threshold, owner, approver và evidence
path. Không tự phát minh threshold để tạo kết quả xanh. Threshold phải lấy từ SLA/ADR
đã phê duyệt; nếu chưa có, PR gate phải thêm quyết định máy đọc được và được owner
duyệt trước lần qualifying run. Run trên environment không khớp phải ghi
`targetMatch=false` và không đóng Phase 1C.

Report chứa ít nhất git SHA, environment ID, test manifest, leakage count, revoke
measurement, quota drift, fairness metric, scan summary và disposition của mọi finding
high/critical.

PR 5 định nghĩa environment profile `phase1c-multi-org-poc` và cập nhật schema/
validator của gate registry cho `G1C-*` cùng `failureDisposition: block-phase-1c`.
Qualifying configuration là chính profile này: ít nhất 2 org, dedicated
`MARKHAND_WORKER_DATABASE_URL`, local/mock embedding theo disposition ở §2.2 và
`targetMatch=true`.

Final report có bảng ánh xạ từng item P1C.8 tới gate row hoặc evidence link, bao gồm
token rotation/reuse/revoke và Qdrant timeout/partial failure; không chỉ báo các metric
load mới.

## 4. Phân chia PR

Các PR chạy nối tiếp để tránh conflict tại `openapi.yaml`, `auth/permissions.rs`,
`db/search.rs`, migration manifest và CI workflow.

### PR 1 — RBAC foundation và catalog truth

Phạm vi:

- xác minh và đóng acceptance 1C-01/1C-02 bằng test đã có;
- thêm canonical RBAC fixture và consistency tests cho DB/OpenAPI/web;
- disposition matrix divergence, audit-retention risk và embedding-token condition
  trong phase plan, issue catalog và risk register;
- đóng 1C-03 chỉ sau fixture validator, DB matrix và operation references cùng xanh.

Exit:

- fast checks và `rust-integration` xanh;
- UI không chứa permission matrix độc lập;
- issue 1C-01/02/03 có evidence cụ thể và chuyển `Done`.

### PR 2 — ACL resolver, predicates và invalidation

Phạm vi:

- implement explicit user/group/role grant semantics;
- enforce read/write/admin access-level ordering và no-dormant-grant invariant;
- đồng bộ resolver, SQL predicate và downstream scope;
- thêm trigger/version bump cho mọi ACL mutation;
- sửa containment mutation order và thêm migration preflight cho dormant grants;
- thêm tests grant, revoke, suspend, cache invalidation và stale-scope defense;
- đóng 1C-05/1C-06 khi kết quả resolver và SQL tương đương.

Exit:

- test RED chứng minh groups hiện không resolve trước implementation;
- 1C-03 dependency đã `Done` bằng evidence PR 1;
- test grant/revoke, containment, concurrent grant-vs-flip, access-level và
  visibility-flip xanh ở fast và DB-gated layer;
- không query collection-scoped nào bỏ shared ACL predicate.

### PR 3 — Guard inventory và operational identities

Phạm vi:

- machine-check guard inventory;
- dual-layer allow/deny cho mọi permission active;
- least-privilege worker/reconcile identity;
- provision `markhand_worker` trong POC deployment;
- audit coverage cho mutation hiện hữu;
- xác minh và cập nhật evidence 1C-04/07/08/09/10/11;
- kiểm qualifying config cấm cloud/shared embedding khi chưa có token metering;
- giữ accepted-risk audit retention hoặc implement purge nếu owner không duyệt defer.

Exit:

- direct-service misuse bị deny;
- route HTTP giữ đúng 403/404 contract và không tạo existence oracle;
- config/static test chứng minh profile G1C yêu cầu worker URL riêng;
- 1C-04/07/09/10 có thể đóng bằng CI evidence;
- 1C-08 giữ deployed half-gate tới PR 5 chứng minh process thật không fallback;
- 1C-11 chỉ đóng khi audit coverage xanh và accepted-risk retention đã được owner duyệt
  hoặc retention implementation đã xanh.

### PR 4 — Multi-org denial suite

Phạm vi:

- shared fixture và denial manifest;
- gom/tái sử dụng test rải rác;
- bổ sung HTTP indexed FTS/Q&A, duplicate-name, org-switch/cache và các gap thực;
- thêm CI invocation riêng và `MARKHAND_TEST_REQUIRED=1`; helper phải panic thay vì
  soft-skip khi required mode thiếu env/dependency.

Exit:

- fixture đạt 2 org và ít nhất 3 user/org;
- mọi manifest row có executable test hoặc `N/A` có bằng chứng;
- guard/route inventory không có operation thiếu denial row;
- zero foreign marker/metadata leak;
- `rust-integration` xanh và artifact manifest được lưu;
- 1C-12 đạt CI half-gate; deployed half-gate còn chờ PR 5.

### PR 5 — Phase 1C security/load gate

Phạm vi:

- registry `G1C-*`, harness, report validator và opt-in CI job;
- boot profile `phase1c-multi-org-poc` với dedicated worker role;
- chạy denial, revoke, quota recovery, fairness, audit và vulnerability scan;
- cập nhật risk register và final phase report.

Exit:

- report có `targetMatch=true`;
- leakage bằng 0;
- mọi metric đạt threshold đã phê duyệt;
- không còn high/critical finding chưa disposition;
- report chứng minh worker process dùng dedicated URL/role, hoàn tất half-gate 1C-08;
- report ánh xạ đủ mọi item P1C.8;
- 1C-12 deployed half-gate và 1C-13 cùng xanh;
- Phase 1C chuyển 13/13 `Done`.

## 5. Luồng implement/review

Mỗi PR dùng vòng lặp:

```text
coordinator task brief + acceptance/RED test
→ implementer TDD
→ reviewer độc lập đọc diff, test và contract
→ implementer sửa finding
→ reviewer xác nhận APPROVED
→ coordinator verify, commit, push, tạo/cập nhật draft PR
→ GitHub CI
→ sửa tới khi required checks xanh
→ cập nhật catalog bằng evidence
```

Grok và Composer luân phiên vai trò implementer/reviewer giữa các PR. Reviewer không
chỉ đọc summary của implementer; phải kiểm diff và chạy lớp test có thể chạy trong
environment của mình. Finding security mức Important trở lên chặn merge. Concern được
ghi rõ disposition nếu cố ý defer.

Mỗi PR là một branch riêng từ `master` mới nhất. PR sau chỉ bắt đầu implementation khi
PR trước đã merge hoặc được rebase trên commit đã merge, nhằm tránh review dựa trên
substrate chưa ổn định.

## 6. Chiến lược kiểm thử

### 6.1 Fast/hermetic trên Cloud Agent

- unit tests cho resolver, cache policy, guard inventory, manifest validator và report
  validator;
- source-shape tests cho shared SQL predicate và route/service registration;
- OpenAPI/codegen/web tests khi contract thay đổi;
- Rust formatting, metadata locked và dependency policy checks trước mỗi push.

### 6.2 GitHub `rust-integration`

- PostgreSQL FORCE RLS và application role;
- MinIO/Qdrant ownership, timeout và capability tests;
- worker pipeline, ACL cache, quota race và multi-org denial fixture;
- job `rust-integration` và G1C harness set `MARKHAND_TEST_REQUIRED=1`; shared
  `take_live`/dependency helpers phải panic khi required mode thiếu biến hoặc service;
- ngoài required mode, ignored test có thể skip rõ ràng cho developer không chạy
  services; không `return Ok(())` tạo false green trong CI.

### 6.3 Deployed gate

- dùng POC compose hoặc environment tương đương có nhiều org;
- profile `phase1c-multi-org-poc` và dedicated worker DB role;
- opt-in qua workflow dispatch/label để không làm chậm mọi PR;
- upload artifact kể cả khi fail và luôn teardown;
- nếu GitHub runner không đáp ứng target environment, owner chạy cùng harness trên máy
  được phê duyệt và commit report đã sanitize.

## 7. Error handling và an toàn migration

- Migration chỉ expand/backfill/contract theo compatibility window; không sửa migration
  lịch sử.
- ACL/permission lookup lỗi phải deny, không fallback scope rộng.
- Test fixture cleanup phải await mọi lane và chỉ báo pass sau khi resource được xóa.
- Gate harness phải thu thập tất cả failure trước khi thoát để không che lỗi thứ hai.
- Report không chứa password, token, document content, prompt, signed capability hoặc
  PII; chỉ chứa marker/hash và aggregate metric.
- Nếu PR phát hiện acceptance cũ mâu thuẫn hành vi đúng, sửa plan và test theo bằng
  chứng trong cùng PR; không đổi production behavior chỉ để khớp một expectation stale.

## 8. Điều kiện hoàn tất

Thiết kế được coi là thực thi xong khi:

1. năm PR hoặc các PR tương đương đã merge theo dependency;
2. 1C-01…1C-13 đều có evidence link và trạng thái `Done`;
3. fast CI, `rust-integration` và deployed `G1C-*` đều xanh;
4. roadmap, issue catalog, generated dashboard và GitHub issue sync nhất quán;
5. không còn blocker high/critical chưa disposition cho trust boundary multi-org;
6. accepted risk audit retention đã được owner duyệt và còn trong phạm vi hiệu lực,
   hoặc retention implementation đã thay thế risk bằng evidence xanh.
