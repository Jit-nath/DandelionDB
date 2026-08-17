from __future__ import annotations

import argparse
import random
import statistics
import time

from dandeliondb import DandelionDB


def vector(rng: random.Random, dimensions: int) -> list[float]:
    values = [rng.uniform(-1.0, 1.0) for _ in range(dimensions)]
    norm = sum(value * value for value in values) ** 0.5
    return [value / norm for value in values]


def main() -> None:
    parser = argparse.ArgumentParser(description="DandelionDB exact/ANN microbenchmark")
    parser.add_argument("--rows", type=int, default=10_000)
    parser.add_argument("--dimensions", type=int, default=384)
    parser.add_argument("--queries", type=int, default=100)
    parser.add_argument("--ann", action="store_true")
    args = parser.parse_args()

    rng = random.Random(7)
    records = [
        {"id": str(index), "vector": vector(rng, args.dimensions)}
        for index in range(args.rows)
    ]
    queries = [vector(rng, args.dimensions) for _ in range(args.queries)]

    with DandelionDB.in_memory(memory_budget_mb=1024) as database:
        collection = database.create_collection(
            "benchmark", dimension=args.dimensions, metric="cosine"
        )
        insert_started = time.perf_counter()
        collection.upsert_many(records)
        insert_seconds = time.perf_counter() - insert_started
        if args.ann:
            build_started = time.perf_counter()
            collection.build_index()
            build_seconds = time.perf_counter() - build_started
        else:
            build_seconds = 0.0

        mode = "ann" if args.ann else "exact"
        latencies = []
        recalls = []
        for query in queries:
            ground_truth = (
                {
                    result.id
                    for result in collection.query(query, top_k=10, mode="exact")
                }
                if args.ann
                else None
            )
            started = time.perf_counter_ns()
            results = collection.query(query, top_k=10, mode=mode)
            latencies.append((time.perf_counter_ns() - started) / 1_000_000)
            if ground_truth is not None:
                recalls.append(
                    len(ground_truth.intersection(result.id for result in results))
                    / len(ground_truth)
                )

        latencies.sort()
        p95_index = min(len(latencies) - 1, int(len(latencies) * 0.95))
        stats = collection.stats()
        print(f"rows={args.rows} dimensions={args.dimensions} mode={mode}")
        print(f"insert_s={insert_seconds:.3f} build_s={build_seconds:.3f}")
        print(
            f"latency_ms median={statistics.median(latencies):.3f} "
            f"p95={latencies[p95_index]:.3f}"
        )
        if recalls:
            print(f"recall_at_10={statistics.mean(recalls):.4f}")
        print(
            f"allocated_bytes={stats['allocated_bytes']} "
            f"vector_bytes={stats['vector_bytes']}"
        )


if __name__ == "__main__":
    main()
