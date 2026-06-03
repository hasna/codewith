import sys
from pathlib import Path

_EXAMPLES_ROOT = Path(__file__).resolve().parents[1]
if str(_EXAMPLES_ROOT) not in sys.path:
    sys.path.insert(0, str(_EXAMPLES_ROOT))

from _bootstrap import ensure_local_sdk_src, runtime_config

ensure_local_sdk_src()

from codewith import Codewith

with Codewith(config=runtime_config()) as client:
    # Create an initial thread and turn so we have a real thread to resume.
    original = client.thread_start(model="gpt-5.4", config={"model_reasoning_effort": "high"})
    first = original.turn("Tell me one fact about Saturn.").run()
    print("Created thread:", original.id)

    # Resume the existing thread by ID.
    resumed = client.thread_resume(original.id)
    second = resumed.turn("Continue with one more fact.").run()
    print(second.final_response)
