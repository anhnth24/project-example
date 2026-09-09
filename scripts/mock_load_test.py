#!/usr/bin/env python3
"""
Mock load test script simulating concurrent requests to measure latency, throughput, and error rate.
Used for Production Readiness assessment (audit only / mock scenario).
"""

import asyncio
import time
import random
import statistics
import json
from typing import List, Dict, Any

async def simulate_request(req_id: int, base_latency_ms: float = 45.0, error_probability: float = 0.005) -> Dict[str, Any]:
    # Simulate jitter and occasional long tail latency (e.g. GC or cache miss)
    jitter = random.gauss(0, 15)
    # Long tail spike (e.g., 2% of requests experience DB lock / vector search latency)
    spike = random.uniform(150, 400) if random.random() < 0.02 else 0.0
    latency = max(5.0, base_latency_ms + jitter + spike)
    
    # Simulate async I/O delay
    await asyncio.sleep(latency / 1000.0)
    
    # Simulate random transient failure
    is_error = random.random() < error_probability
    return {
        "req_id": req_id,
        "latency_ms": latency,
        "success": not is_error
    }

async def run_scenario(concurrency: int, total_requests: int) -> Dict[str, Any]:
    start_time = time.time()
    semaphore = asyncio.Semaphore(concurrency)
    
    async def bounded_req(i: int):
        async with semaphore:
            return await simulate_request(i)
            
    tasks = [bounded_req(i) for i in range(total_requests)]
    results = await asyncio.gather(*tasks)
    total_time = time.time() - start_time
    
    latencies = [r["latency_ms"] for r in results]
    latencies.sort()
    errors = sum(1 for r in results if not r["success"])
    
    p50 = statistics.median(latencies)
    p90 = latencies[int(len(latencies) * 0.90)]
    p95 = latencies[int(len(latencies) * 0.95)]
    p99 = latencies[int(len(latencies) * 0.99)]
    avg_latency = statistics.mean(latencies)
    throughput = len(results) / total_time
    error_rate = (errors / len(results)) * 100.0
    
    return {
        "concurrency": concurrency,
        "total_requests": total_requests,
        "duration_sec": round(total_time, 2),
        "throughput_rps": round(throughput, 2),
        "error_rate_pct": round(error_rate, 2),
        "avg_latency_ms": round(avg_latency, 2),
        "p50_latency_ms": round(p50, 2),
        "p90_latency_ms": round(p90, 2),
        "p95_latency_ms": round(p95, 2),
        "p99_latency_ms": round(p99, 2),
    }

async def main():
    print("Running Mock Load Test Scenarios (10 - 100 concurrent requests)...")
    scenarios = [
        {"concurrency": 10, "requests": 200},
        {"concurrency": 50, "requests": 500},
        {"concurrency": 100, "requests": 1000},
    ]
    
    summary = []
    for sc in scenarios:
        res = await run_scenario(sc["concurrency"], sc["requests"])
        summary.append(res)
        print(f"Concurrency: {res['concurrency']:3d} | RPS: {res['throughput_rps']:6.2f} | P95: {res['p95_latency_ms']:6.2f}ms | P99: {res['p99_latency_ms']:6.2f}ms | Error: {res['error_rate_pct']:.2f}%")

    with open("bench/mock_load_test_results.json", "w", encoding="utf-8") as f:
        json.dump(summary, f, indent=2)
    print("\nSaved results to bench/mock_load_test_results.json")

if __name__ == "__main__":
    asyncio.run(main())
