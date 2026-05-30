from src.db import DandelionDB
from src.table import Table, col, vector


# table
class userPDF(Table):
    id = col(int, primary=True, auto_increment=True)
    vector = col(vector, n_dim=3, nullable=False)
    metadata = col(str, default="")


db = DandelionDB(file_path="db.lion", tables=[userPDF])


print(db.tables())

for item in db.tables():
    print(db.schema(item))

