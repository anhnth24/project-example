#!/usr/bin/env python3
"""
Benchmark double-fold tsvector cost analysis (Issue #437).
Compares:
  1. Pre-0037 (Single fold)
  2. 0037 (Unconditional double fold)
  3. Conditional fold (Only double fold when '/' is present)
"""

import hashlib
import json
import os
import random
import sys
import time
import psycopg2
from psycopg2.extras import execute_batch

DB_URL = os.environ.get(
    "MARKHAND_BENCH_DATABASE_URL",
    "postgres://markhand:markhand_dev_only@127.0.0.1:54329/markhand_bench",
)
CORPUS_SIZE = int(os.environ.get("CORPUS_SIZE", "10000"))
SLASH_RATIO = float(os.environ.get("SLASH_RATIO", "0.20"))  # 20% chunks contain slashes
SEED = 42

SAMPLE_TEXTS = [
    "Căn cứ Nghị định số {doc_num} quy định về chuyển đổi số quốc gia và an toàn thông tin mạng.",
    "Báo cáo tình hình triển khai hệ thống quản trị dữ liệu tập trung trong quý {quarter} năm {year}.",
    "Theo hướng dẫn tại văn bản {doc_num} ngày {date}, các đơn vị cần đối soát số liệu giao dịch định kỳ.",
    "Quy trình xác thực người dùng và phân quyền truy cập tuân thủ tiêu chuẩn an toàn cấp độ 3.",
    "Tổng hợp danh sách các dự án công nghệ thông tin đã nghiệm thu trong giai đoạn {year_range}.",
    "Kế hoạch nâng cấp hạ tầng máy chủ và hệ thống lưu trữ phân tán cho trung tâm dữ liệu.",
    "Biên bản họp kỹ thuật về việc tích hợp API thanh toán điện tử và đối soát tự động ngày {date}.",
    "Thông tư {doc_num} hướng dẫn chi tiết về cấu trúc dữ liệu trao đổi giữa các bộ ban ngành.",
    "Đánh giá hiệu năng tìm kiếm toàn văn và trích xuất thông tin từ tài liệu văn bản tiếng Việt.",
    "Quyết định số {doc_num} ban hành quy chế bảo mật dữ liệu và phòng chống tấn công mạng.",
    "Nghiên cứu kiến trúc mô hình ngôn ngữ lớn phục vụ trợ lý ảo hỗ trợ công chức giải quyết thủ tục.",
    "Quy chế phối hợp giữa các đơn vị nghiệp vụ trong việc xử lý sự cố an toàn thông tin ngày {date}.",
]

SAMPLE_NO_SLASH = [
    "Hệ thống quản lý tài liệu và văn bản số hóa cung cấp khả năng tìm kiếm nhanh và chính xác.",
    "Kiến trúc microservices giúp tăng tính sẵn sàng và khả năng mở rộng của dịch vụ lõi.",
    "Quy chuẩn kỹ thuật quốc gia về cấu trúc thông điệp dữ liệu trao đổi trong cổng thông tin.",
    "Báo cáo tài chính và tình hình sử dụng ngân sách cho các đề án khoa học công nghệ.",
    "Tài liệu hướng dẫn cài đặt và vận hành cụm cơ sở dữ liệu PostgreSQL cho môi trường sản xuất.",
    "Đặc tả yêu cầu phần mềm cho phân hệ quản lý người dùng và nhóm quyền truy cập theo vai trò.",
    "Quy trình kiểm thử hồi quy và đánh giá độ tin cậy của phần mềm trước khi phát hành phiên bản mới.",
    "Phân tích lưu lượng truy cập và giám sát hiệu năng dịch vụ thông qua hệ thống OpenTelemetry.",
    "Chính sách bảo mật thông tin cá nhân và quản lý dữ liệu người dùng trên môi trường đám mây.",
    "Tối ưu hóa chỉ mục tìm kiếm và giảm độ trễ truy vấn đối với cơ sở dữ liệu quy mô lớn.",
]


def generate_corpus(size=10000, slash_ratio=0.20):
    rng = random.Random(SEED)
    chunks = []
    slash_count = int(size * slash_ratio)

    doc_ids = ["1502/CV-CNTT", "88/QĐ-CNTT", "12/2026/TT-BTTTT", "45/NQ-CP", "102/TB-VPCP", "204/BC-KHTC"]
    dates = ["27/08/2026", "15/09/2025", "01/01/2024", "30/04/1975", "12/12/2023", "19/08/2026"]

    for i in range(size):
        has_slash = i < slash_count
        num_sentences = rng.randint(3, 5)
        sentences = []
        if has_slash:
            tmpl = rng.choice(SAMPLE_TEXTS)
            doc_num = rng.choice(doc_ids)
            date = rng.choice(dates)
            sentences.append(tmpl.format(
                doc_num=doc_num,
                date=date,
                quarter=rng.randint(1, 4),
                year=rng.randint(2023, 2026),
                year_range=f"{rng.randint(2021, 2023)}-{rng.randint(2024, 2026)}",
            ))
            for _ in range(num_sentences - 1):
                sentences.append(rng.choice(SAMPLE_NO_SLASH))
            heading = [f"Chương {i % 10}", "Văn bản hành chính", f"Điều {i % 50 + 1}"]
        else:
            for _ in range(num_sentences):
                sentences.append(rng.choice(SAMPLE_NO_SLASH))
            heading = [f"Phần {i % 8}", "Tài liệu kỹ thuật", f"Mục {i % 30 + 1}"]

        body = f"Khoản {i % 20 + 1}. " + " ".join(sentences) + f" (Mã lưu trữ nội bộ ID-{i:06d})."

        ident = hashlib.sha256(f"chunk-bench-{i}-{body}".encode("utf-8")).hexdigest()
        chunks.append({
            "ordinal": i,
            "heading_path": heading,
            "body": body,
            "identity": ident,
            "has_slash": has_slash,
        })
    return chunks


TRIGGER_V16_SINGLE = """
CREATE OR REPLACE FUNCTION chunks_set_tsv()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    NEW.tsv := to_tsvector(
        'simple',
        markhand_accent_fold(
            coalesce(array_to_string(NEW.heading_path, ' '), '') || ' ' || NEW.body
        )
    );
    RETURN NEW;
END;
$$;
"""

TRIGGER_V37_DOUBLE = """
CREATE OR REPLACE FUNCTION chunks_set_tsv()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    folded text;
BEGIN
    folded := markhand_accent_fold(
        coalesce(array_to_string(NEW.heading_path, ' '), '') || ' ' || NEW.body
    );
    NEW.tsv := to_tsvector('simple', folded)
        || to_tsvector('simple', translate(folded, '/', ' '));
    RETURN NEW;
END;
$$;
"""

TRIGGER_CONDITIONAL = """
CREATE OR REPLACE FUNCTION chunks_set_tsv()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    folded text;
BEGIN
    folded := markhand_accent_fold(
        coalesce(array_to_string(NEW.heading_path, ' '), '') || ' ' || NEW.body
    );
    IF position('/' in folded) > 0 THEN
        NEW.tsv := to_tsvector('simple', folded)
            || to_tsvector('simple', translate(folded, '/', ' '));
    ELSE
        NEW.tsv := to_tsvector('simple', folded);
    END IF;
    RETURN NEW;
END;
$$;
"""

QUERIES = [
    ("Slash sub-token '1502'", "to_tsquery('simple', '1502')"),
    ("Slash sub-token 'cv-cntt'", "to_tsquery('simple', 'cv-cntt')"),
    ("Date sub-token '27 & 08'", "to_tsquery('simple', '27 & 08')"),
    ("Regular phrase 'van & ban'", "to_tsquery('simple', 'van & ban')"),
    ("Regular phrase 'an & toan'", "to_tsquery('simple', 'an & toan')"),
    ("Multi-word 'he & thong & quan & ly'", "to_tsquery('simple', 'he & thong & quan & ly')"),
]


def percentile(data, p):
    if not data:
        return 0.0
    sorted_data = sorted(data)
    k = (len(sorted_data) - 1) * (p / 100.0)
    f = int(k)
    c = min(f + 1, len(sorted_data) - 1)
    d = k - f
    return sorted_data[f] + d * (sorted_data[c] - sorted_data[f])


def setup_corpus(conn, chunks):
    cur = conn.cursor()
    cur.execute("SET app.org_id = '11111111-1111-1111-1111-111111111111';")
    org_id = "11111111-1111-1111-1111-111111111111"
    user_id = "22222222-2222-2222-2222-222222222201"
    collection_id = "55555555-5555-5555-5555-555555555501"

    doc_id = "66666666-6666-6666-6666-666666666601"
    version_id = "77777777-7777-7777-7777-777777777701"
    index_meta_id = "88888888-8888-8888-8888-888888888801"
    sig = hashlib.sha256(b"bench-signature").hexdigest()
    content_sha = hashlib.sha256(b"bench-content").hexdigest()

    cur.execute("ALTER TABLE chunks NO FORCE ROW LEVEL SECURITY;")
    cur.execute("DELETE FROM chunks WHERE org_id = %s;", (org_id,))
    cur.execute("ALTER TABLE chunks FORCE ROW LEVEL SECURITY;")

    cur.execute(
        """
        INSERT INTO documents (id, org_id, collection_id, title, state, created_by_user_id)
        VALUES (%s, %s, %s, 'Benchmark Corpus Document', 'uploaded', %s)
        ON CONFLICT (id) DO NOTHING;
        """,
        (doc_id, org_id, collection_id, user_id),
    )

    cur.execute(
        """
        INSERT INTO document_versions (
            id, org_id, document_id, version_number, publication_state, is_current,
            content_sha256, original_object_key, effective_from, created_by_user_id
        ) VALUES (%s, %s, %s, 1, 'published', true, %s, 'bench.md', now(), %s)
        ON CONFLICT (id) DO NOTHING;
        """,
        (version_id, org_id, doc_id, content_sha, user_id),
    )
    cur.execute("UPDATE documents SET current_version_id = %s WHERE id = %s;", (version_id, doc_id))

    cur.execute(
        """
        INSERT INTO index_metadata (
            id, org_id, collection_id, index_signature_sha256, embedding_family,
            embedding_revision, dimensions, runtime_path, generation, is_active, state
        ) VALUES (%s, %s, %s, %s, 'f', 'r', 8, 'local-hash', 1, true, 'active')
        ON CONFLICT (id) DO NOTHING;
        """,
        (index_meta_id, org_id, collection_id, sig),
    )
    conn.commit()

    print(f"Ingesting {len(chunks)} chunks into PostgreSQL...")
    start_t = time.perf_counter()
    insert_sql = """
        INSERT INTO chunks (
            org_id, document_id, version_id, ordinal, heading_path, body,
            chunk_identity_sha256, index_metadata_id, index_signature
        ) VALUES (
            %s, %s, %s, %s, %s, %s, %s, %s, %s
        );
    """
    rows = [
        (
            org_id,
            doc_id,
            version_id,
            c["ordinal"],
            c["heading_path"],
            c["body"],
            c["identity"],
            index_meta_id,
            sig,
        )
        for c in chunks
    ]
    cur.execute("ALTER TABLE chunks NO FORCE ROW LEVEL SECURITY;")
    execute_batch(cur, insert_sql, rows, page_size=1000)
    cur.execute("ALTER TABLE chunks FORCE ROW LEVEL SECURITY;")
    conn.commit()
    elapsed = time.perf_counter() - start_t
    print(f"Ingestion done in {elapsed:.2f}s")
def evaluate_scenario(conn, name, trigger_sql, runs=10, warmup=3):
    print(f"\n==================== Evaluating: {name} ====================")
    cur = conn.cursor()
    cur.execute("SET app.org_id = '11111111-1111-1111-1111-111111111111';")

    cur.execute(trigger_sql)
    conn.commit()

    print("Recalculating chunks.tsv (backfill)...")
    t0 = time.perf_counter()
    cur.execute("ALTER TABLE chunks NO FORCE ROW LEVEL SECURITY;")
    cur.execute("UPDATE chunks SET body = body;")
    cur.execute("ALTER TABLE chunks FORCE ROW LEVEL SECURITY;")
    conn.commit()
    t_recalc = time.perf_counter() - t0
    print(f"Recalculation took {t_recalc:.2f}s")

    print("Running VACUUM FULL & REINDEX to obtain deterministic disk size...")
    old_autocommit = conn.autocommit
    conn.autocommit = True
    cur.execute("VACUUM FULL chunks;")
    cur.execute("REINDEX TABLE chunks;")
    cur.execute("ANALYZE chunks;")
    conn.autocommit = old_autocommit

    cur.execute("""
        SELECT
            pg_relation_size('idx_chunks__tsv') as gin_size,
            pg_total_relation_size('idx_chunks__tsv') as gin_total_size,
            pg_relation_size('chunks') as chunks_table_size,
            pg_total_relation_size('chunks') as chunks_total_size,
            sum(length(tsv)) as total_lexemes,
            avg(length(tsv)) as avg_lexemes_per_chunk,
            sum(pg_column_size(tsv)) as total_tsv_bytes,
            avg(pg_column_size(tsv)) as avg_tsv_bytes
        FROM chunks;
    """)
    row = cur.fetchone()
    gin_size = row[0]
    gin_total_size = row[1]
    chunks_table_size = row[2]
    chunks_total_size = row[3]
    total_lexemes = row[4]
    avg_lexemes = float(row[5]) if row[5] else 0.0
    total_tsv_bytes = row[6]
    avg_tsv_bytes = float(row[7]) if row[7] else 0.0

    print(f"GIN Index Size: {gin_size / (1024*1024):.2f} MB ({gin_size:,} bytes)")
    print(f"Chunks Table Total Size: {chunks_total_size / (1024*1024):.2f} MB ({chunks_total_size:,} bytes)")
    print(f"Total Lexemes: {total_lexemes:,} (avg {avg_lexemes:.1f} per chunk)")
    print(f"Stored TSV Column Size: {total_tsv_bytes / (1024*1024):.2f} MB (avg {avg_tsv_bytes:.1f} bytes per chunk)")

    query_results = []
    for q_label, q_tsquery in QUERIES:
        sql = f"EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) SELECT count(*) FROM chunks WHERE tsv @@ {q_tsquery};"

        for _ in range(warmup):
            cur.execute(sql)
            cur.fetchone()

        times = []
        hits = 0
        for _ in range(runs):
            cur.execute(sql)
            res = cur.fetchone()[0]
            exec_time_ms = float(res[0]["Execution Time"])
            times.append(exec_time_ms)
            if hits == 0:
                cur.execute(f"SELECT count(*) FROM chunks WHERE tsv @@ {q_tsquery};")
                hits = cur.fetchone()[0]

        p50 = percentile(times, 50)
        p95 = percentile(times, 95)
        mean = sum(times) / len(times)
        min_t = min(times)
        max_t = max(times)
        print(f"  [{q_label}] matches: {hits} | p50: {p50:.3f}ms | p95: {p95:.3f}ms | mean: {mean:.3f}ms")

        query_results.append({
            "label": q_label,
            "hits": hits,
            "p50_ms": p50,
            "p95_ms": p95,
            "mean_ms": mean,
            "min_ms": min_t,
            "max_ms": max_t,
            "raw_ms": times,
        })

    return {
        "scenario": name,
        "gin_size_bytes": gin_size,
        "gin_size_mb": gin_size / (1024 * 1024),
        "table_size_bytes": chunks_table_size,
        "table_total_size_mb": chunks_total_size / (1024 * 1024),
        "total_lexemes": total_lexemes,
        "avg_lexemes_per_chunk": avg_lexemes,
        "total_tsv_bytes": total_tsv_bytes,
        "avg_tsv_bytes": avg_tsv_bytes,
        "recalc_seconds": t_recalc,
        "queries": query_results,
    }

def main():
    print(f"Connecting to {DB_URL}...")
    conn = psycopg2.connect(DB_URL)

    chunks = generate_corpus(size=CORPUS_SIZE, slash_ratio=SLASH_RATIO)
    slash_chunks = sum(1 for c in chunks if c["has_slash"])
    print(f"Generated corpus: {len(chunks)} chunks ({slash_chunks} with slashes, {slash_chunks/len(chunks)*100:.1f}%)")

    setup_corpus(conn, chunks)

    res_single = evaluate_scenario(conn, "Pre-0037 (Single fold)", TRIGGER_V16_SINGLE)
    res_double = evaluate_scenario(conn, "Migration 0037 (Unconditional double fold)", TRIGGER_V37_DOUBLE)
    res_cond = evaluate_scenario(conn, "Proposed: Conditional fold (when '/' present)", TRIGGER_CONDITIONAL)

    print("\nRestoring 0037 trigger state to leave database clean...")
    cur = conn.cursor()
    cur.execute(TRIGGER_V37_DOUBLE)
    cur.execute("ALTER TABLE chunks NO FORCE ROW LEVEL SECURITY;")
    cur.execute("UPDATE chunks SET body = body;")
    cur.execute("ALTER TABLE chunks FORCE ROW LEVEL SECURITY;")
    conn.commit()

    conn.close()

    results = {
        "corpus_size": CORPUS_SIZE,
        "slash_ratio": SLASH_RATIO,
        "scenarios": [res_single, res_double, res_cond],
    }

    out_json = "bench/tsv_cost_benchmark_results.json"
    with open(out_json, "w", encoding="utf-8") as f:
        json.dump(results, f, indent=2, ensure_ascii=False)
    print(f"\nWrote full benchmark results to {out_json}")


if __name__ == "__main__":
    main()

