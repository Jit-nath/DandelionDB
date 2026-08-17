import struct
from dataclasses import dataclass


@dataclass
class Person:
    id: int
    name: str


def serialize(person: Person) -> bytes:
    name = person.name.encode("utf-8")

    return (
        struct.pack("<I", person.id) +
        struct.pack("<H", len(name)) +
        name
    )


person1 = Person(1, "Jit Debnath")
person2 = Person(2, "Arpan Maji")
person3 = Person(3, "Prasun Pati")
person4 = Person(
    4,
    "This is a much longer name than before because it is no longer limited to 32 bytes."
)

collection_person = [person1, person2, person3, person4]


if __name__ == "__main__":
    with open("database.lion", "wb") as f:
        for person in collection_person:
            record = serialize(person)
            f.write(record)
