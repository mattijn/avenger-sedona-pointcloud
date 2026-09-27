from typing import Any, Literal, Optional, TypedDict

class Issue(TypedDict):
    layer: Literal[0, 1, 2, 3, 4]
    step: Optional[int]
    name: Optional[str]
    arg: Optional[str]
    message: str
    span: Optional[tuple[int, int]]

class Report(TypedDict):
    valid: bool
    steps: int
    layers: list[int]
    issues: list[Issue]

class Validator:
    def __init__(self, schema: Optional[list[tuple[str, str]]] = None, data: bool = True) -> None: ...
    def check(self, text: str) -> Report: ...
    def check_many(self, texts: list[str]) -> list[Report]: ...
    def check_steps(self, steps: Any) -> Report: ...
    def parse(self, text: str) -> str: ...

def export_cel() -> str: ...
def spec_json() -> str: ...

__version__: str
