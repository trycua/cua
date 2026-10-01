# Source: https://github.com/QwenLM/Qwen-Agent/blob/main/qwen_agent/llm/schema.py

from typing import List, Literal, Optional, Tuple, Union

from pydantic import BaseModel, field_validator, model_validator


class BaseModelCompatibleDict(BaseModel):
    def __getitem__(self, item):
        return getattr(self, item)

    def __setitem__(self, key, value):
        setattr(self, key, value)

    def model_dump(self, **kwargs):
        if "exclude_none" not in kwargs:
            kwargs["exclude_none"] = True
        return super().model_dump(**kwargs)

    def model_dump_json(self, **kwargs):
        if "exclude_none" not in kwargs:
            kwargs["exclude_none"] = True
        return super().model_dump_json(**kwargs)

    def get(self, key, default=None):
        try:
            return getattr(self, key)
        except AttributeError:
            return default

    def __str__(self):
        return f"{self.model_dump()}"


class FunctionCall(BaseModelCompatibleDict):
    name: str
    arguments: str

    def __init__(self, name: str, arguments: str):
        super().__init__(name=name, arguments=arguments)

    def __repr__(self):
        return f"FunctionCall({self.model_dump()})"


class ContentItem(BaseModelCompatibleDict):
    text: Optional[str] = None
    image: Optional[str] = None
    file: Optional[str] = None
    audio: Optional[Union[str, dict]] = None
    video: Optional[Union[str, list]] = None

    def __init__(
        self,
        text: Optional[str] = None,
        image: Optional[str] = None,
        file: Optional[str] = None,
        audio: Optional[Union[str, dict]] = None,
        video: Optional[Union[str, list]] = None,
    ):
        super().__init__(text=text, image=image, file=file, audio=audio, video=video)

    @model_validator(mode="after")
    def check_exclusivity(self):
        provided_fields = 0
        if self.text is not None:
            provided_fields += 1
        if self.image:
            provided_fields += 1
        if self.file:
            provided_fields += 1
        if self.audio:
            provided_fields += 1
        if self.video:
            provided_fields += 1

        if provided_fields != 1:
            raise ValueError(
                "Exactly one of 'text', 'image', 'file', 'audio', or 'video' must be provided."
            )
        return self

    def __repr__(self):
        return f"ContentItem({self.model_dump()})"

    def get_type_and_value(
        self,
    ) -> Tuple[Literal["text", "image", "file", "audio", "video"], str]:
        ((t, v),) = self.model_dump().items()
        assert t in ("text", "image", "file", "audio", "video")
        return t, v

    @property
    def type(self) -> Literal["text", "image", "file", "audio", "video"]:
        t, _ = self.get_type_and_value()
        return t

    @property
    def value(self) -> str:
        _, v = self.get_type_and_value()
        return v


class Message(BaseModelCompatibleDict):
    role: str
    content: Union[str, List[ContentItem]]
    reasoning_content: Optional[Union[str, List[ContentItem]]] = None
    name: Optional[str] = None
    function_call: Optional[FunctionCall] = None
    extra: Optional[dict] = None

    def __init__(
        self,
        role: str,
        content: Union[str, List[ContentItem]],
        reasoning_content: Optional[Union[str, List[ContentItem]]] = None,
        name: Optional[str] = None,
        function_call: Optional[FunctionCall] = None,
        extra: Optional[dict] = None,
        **kwargs,
    ):
        if content is None:
            content = ""
        if reasoning_content is None:
            reasoning_content = ""
        super().__init__(
            role=role,
            content=content,
            reasoning_content=reasoning_content,
            name=name,
            function_call=function_call,
            extra=extra,
        )

    def __repr__(self):
        return f"Message({self.model_dump()})"

    @field_validator("role")
    def role_checker(cls, value: str) -> str:
        values = ["system", "user", "assistant", "function"]
        if value not in values:
            raise ValueError(f'{value} must be one of {",".join(values)}')
        return value
