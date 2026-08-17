import struct
from dataclasses import dataclass


@dataclass
class Person:
    id: int
    name: str


def deserialize(file) -> Person | None:
    # Read the ID
    raw = file.read(4)
    if not raw:
        return None  # EOF

    person_id = struct.unpack("<I", raw)[0]

    # Read the name length
    raw = file.read(2)
    if len(raw) != 2:
        raise ValueError("Unexpected end of file while reading name length")

    name_length = struct.unpack("<H", raw)[0]

    # Read the name
    raw = file.read(name_length)
    if len(raw) != name_length:
        raise ValueError("Unexpected end of file while reading name")

    name = raw.decode("utf-8")

    return Person(person_id, name)


if __name__ == "__main__":
    with open("database.lion", "rb") as f:
        while True:
            person = deserialize(f)

            if person is None:
                break

            print(person)
