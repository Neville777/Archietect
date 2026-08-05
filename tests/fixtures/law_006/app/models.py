from sqlmodel import SQLModel

class ItemBase(SQLModel):
    title: str

class Item(ItemBase, table=True):
    id: int
