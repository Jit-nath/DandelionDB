from dandeliondb import DandelionDB

with DandelionDB.in_memory() as database:
    documents = database.create_collection(
        "documents",
        dimension=3,
        metric="cosine",
        filter_fields=["kind"],
    )
    documents.upsert_many(
        [
            {
                "id": "intro",
                "vector": [1.0, 0.0, 0.0],
                "metadata": {"kind": "guide", "title": "Introduction"},
            },
            {
                "id": "reference",
                "vector": [0.8, 0.2, 0.0],
                "metadata": {"kind": "manual", "title": "Reference"},
            },
        ]
    )

    for result in documents.query(
        [1.0, 0.0, 0.0],
        top_k=2,
        filter={"kind": {"$in": ["guide", "manual"]}},
    ):
        print(result)
