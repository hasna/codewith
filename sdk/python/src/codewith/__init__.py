"""Python SDK source preview for app-server workflows.

The default development setup runs the pinned upstream Codex runtime from
``openai-codex-cli-bin``. To experiment with Codewith, configure its executable
explicitly::

    from codewith import CodexConfig, Codewith, Sandbox

    config = CodexConfig(codex_bin="/absolute/path/to/codewith")
    with Codewith(config=config) as client:
        thread = client.thread_start(sandbox=Sandbox.workspace_write)
        result = thread.run("Describe this project.")
        print(result.final_response)
"""

from ._version import __version__
from .api import (
    ApprovalMode,
    AsyncChatgptLoginHandle,
    AsyncCodewith,
    AsyncDeviceCodeLoginHandle,
    AsyncThread,
    AsyncTurnHandle,
    ChatgptLoginHandle,
    Codewith,
    DeviceCodeLoginHandle,
    ImageInput,
    Input,
    InputItem,
    LocalImageInput,
    MentionInput,
    RunInput,
    Sandbox,
    SkillInput,
    TextInput,
    Thread,
    TurnHandle,
    TurnResult,
)
from .client import CodexConfig
from .errors import (
    CodexError,
    CodexRpcError,
    InternalRpcError,
    InvalidParamsError,
    InvalidRequestError,
    JsonRpcError,
    MethodNotFoundError,
    ParseError,
    RetryLimitExceededError,
    ServerBusyError,
    TransportClosedError,
    is_retryable_error,
)
from .retry import retry_on_overload

__all__ = [
    "__version__",
    "CodexConfig",
    "Codewith",
    "AsyncCodewith",
    "ApprovalMode",
    "Sandbox",
    "ChatgptLoginHandle",
    "DeviceCodeLoginHandle",
    "AsyncChatgptLoginHandle",
    "AsyncDeviceCodeLoginHandle",
    "Thread",
    "AsyncThread",
    "TurnHandle",
    "AsyncTurnHandle",
    "TurnResult",
    "Input",
    "InputItem",
    "RunInput",
    "TextInput",
    "ImageInput",
    "LocalImageInput",
    "SkillInput",
    "MentionInput",
    "retry_on_overload",
    "CodexError",
    "TransportClosedError",
    "JsonRpcError",
    "CodexRpcError",
    "ParseError",
    "InvalidRequestError",
    "MethodNotFoundError",
    "InvalidParamsError",
    "InternalRpcError",
    "ServerBusyError",
    "RetryLimitExceededError",
    "is_retryable_error",
]
