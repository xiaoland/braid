"""Strict configuration models for transport-owned runtime state."""

from __future__ import annotations

from ipaddress import IPv4Address, IPv6Address
from collections.abc import Mapping
import os
from pathlib import Path
from typing import Annotated, Literal, Self

from pydantic import (
    BaseModel,
    ConfigDict,
    Field,
    IPvAnyAddress,
    StringConstraints,
    ValidationError,
    field_validator,
    model_validator,
)

EnvironmentVariableName = Annotated[
    str,
    StringConstraints(
        strict=True,
        strip_whitespace=True,
        min_length=1,
        pattern=r"^[A-Z_][A-Z0-9_]*$",
    ),
]
Sha256Hex = Annotated[
    str,
    StringConstraints(strict=True, pattern=r"^[0-9a-f]{64}$"),
]
PositiveSeconds = Annotated[
    float,
    Field(strict=True, gt=0, allow_inf_nan=False),
]
TcpPort = Annotated[int, Field(strict=True, ge=1, le=65_535)]


class StrictModel(BaseModel):
    """Base model that rejects coercion and unknown configuration keys."""

    model_config = ConfigDict(extra="forbid", strict=True, frozen=True)


class SecretReference(StrictModel):
    """A reference to secret material; never the material itself."""

    environment: EnvironmentVariableName | None = None
    file: Path | None = None

    @model_validator(mode="after")
    def require_exactly_one_source(self) -> Self:
        if (self.environment is None) == (self.file is None):
            raise ValueError("exactly one of environment or file must be set")
        if self.file is not None and not self.file.is_absolute():
            raise ValueError("secret file reference must be an absolute path")
        return self


class GitHubConfig(StrictModel):
    app_id: Annotated[int, Field(strict=True, gt=0)]
    private_key: SecretReference
    webhook_secret: SecretReference
    agent_login: Annotated[
        str,
        StringConstraints(
            strict=True,
            strip_whitespace=True,
            min_length=1,
            max_length=39,
            pattern=(
                r"^[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?"
                r"(?:\[bot\])?$"
            ),
        ),
    ]
    wrapper_login: Annotated[
        str,
        StringConstraints(
            strict=True,
            strip_whitespace=True,
            min_length=1,
            max_length=39,
            pattern=(
                r"^[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?"
                r"(?:\[bot\])?$"
            ),
        ),
    ]

    @model_validator(mode="after")
    def require_distinct_github_identities(self) -> Self:
        if self.agent_login.casefold() == self.wrapper_login.casefold():
            raise ValueError("Agent and Wrapper GitHub identities must be distinct")
        return self


class IngressConfig(StrictModel):
    host: IPvAnyAddress = IPv4Address("127.0.0.1")
    port: TcpPort = 8_080
    health_port: TcpPort = 8_081

    @field_validator("host")
    @classmethod
    def require_loopback(
        cls, value: IPv4Address | IPv6Address
    ) -> IPv4Address | IPv6Address:
        if not value.is_loopback:
            raise ValueError("ingress host must be a loopback address")
        return value

    @model_validator(mode="after")
    def require_distinct_ports(self) -> Self:
        if self.port == self.health_port:
            raise ValueError("webhook and health ports must be distinct")
        return self


class TimingConfig(StrictModel):
    quiet_window_seconds: PositiveSeconds = 30.0
    mirror_message_count_threshold: Annotated[
        int, Field(strict=True, ge=1, le=1_000)
    ] = 10
    mirror_maximum_dirty_age_seconds: PositiveSeconds = 120.0
    reconciliation_interval_seconds: PositiveSeconds = 60.0
    mirror_comment_bytes: Annotated[
        int, Field(strict=True, ge=1_024, le=65_536)
    ] = 60_000
    mirror_projection_bytes: Annotated[
        int, Field(strict=True, ge=16_384, le=1_048_576)
    ] = 262_144
    mirror_tool_call_bytes: Annotated[
        int, Field(strict=True, ge=256, le=65_536)
    ] = 8_192
    mirror_tool_result_bytes: Annotated[
        int, Field(strict=True, ge=256, le=131_072)
    ] = 16_384

    @model_validator(mode="after")
    def keep_projection_fields_inside_total_bound(self) -> Self:
        if self.mirror_tool_call_bytes >= self.mirror_projection_bytes:
            raise ValueError("tool call bound must be below projection bound")
        if self.mirror_tool_result_bytes >= self.mirror_projection_bytes:
            raise ValueError("tool result bound must be below projection bound")
        return self


class RuntimePaths(StrictModel):
    state_database: Path
    provider_cwd: Path
    provider_writable_roots: tuple[Path, ...] = ()
    provider_sandbox: Literal["read-only", "workspace-write"] = "workspace-write"
    collaboration_instructions: Path

    @model_validator(mode="after")
    def reject_writable_roots_in_read_only_sandbox(self) -> Self:
        if self.provider_sandbox == "read-only" and self.provider_writable_roots:
            raise ValueError("read-only provider sandbox cannot add writable roots")
        return self

    @field_validator("state_database", "provider_cwd", "collaboration_instructions")
    @classmethod
    def require_absolute_path(cls, value: Path) -> Path:
        if not value.is_absolute():
            raise ValueError("runtime path reference must be absolute")
        return value

    @field_validator("provider_writable_roots")
    @classmethod
    def require_absolute_unique_writable_roots(
        cls, value: tuple[Path, ...]
    ) -> tuple[Path, ...]:
        if any(not path.is_absolute() for path in value):
            raise ValueError("provider writable root must be absolute")
        if len(value) != len(set(value)):
            raise ValueError("provider writable roots must not contain duplicates")
        return value


class AppServerConfig(StrictModel):
    executable: Path
    version: Annotated[
        str,
        StringConstraints(strict=True, strip_whitespace=True, min_length=1),
    ]
    stable_schema_sha256: Sha256Hex
    experimental_schema_sha256: Sha256Hex
    environment_allowlist: tuple[EnvironmentVariableName, ...] = ()

    @field_validator("executable")
    @classmethod
    def require_absolute_executable(cls, value: Path) -> Path:
        if not value.is_absolute():
            raise ValueError("app-server executable must be an absolute path")
        return value

    @field_validator("environment_allowlist")
    @classmethod
    def require_unique_environment_names(
        cls, value: tuple[str, ...]
    ) -> tuple[str, ...]:
        if len(value) != len(set(value)):
            raise ValueError("environment allowlist must not contain duplicates")
        return value


class BridgeConfig(StrictModel):
    github: GitHubConfig
    ingress: IngressConfig
    timing: TimingConfig
    paths: RuntimePaths
    app_server: AppServerConfig

    @model_validator(mode="after")
    def keep_wrapper_secrets_out_of_provider_environment(self) -> Self:
        secret_environment_names = {
            reference.environment
            for reference in (
                self.github.private_key,
                self.github.webhook_secret,
            )
            if reference.environment is not None
        }
        exposed_names = secret_environment_names.intersection(
            self.app_server.environment_allowlist
        )
        if exposed_names:
            joined_names = ", ".join(sorted(exposed_names))
            raise ValueError(
                "app-server environment allowlist exposes Wrapper secret "
                f"references: {joined_names}"
            )
        return self


class ConfigLoadError(ValueError):
    """A safe, operator-facing configuration loading failure."""


class SecretLoadError(ValueError):
    """Secret material could not be loaded from its configured authority."""


def load_config(path: Path) -> BridgeConfig:
    """Read and validate a JSON configuration without echoing input values."""

    try:
        raw_config = path.read_bytes()
    except OSError as error:
        raise ConfigLoadError(f"cannot read configuration file: {error}") from error

    try:
        return BridgeConfig.model_validate_json(raw_config)
    except ValidationError as error:
        details = []
        for issue in error.errors(include_input=False, include_url=False):
            location = ".".join(str(part) for part in issue["loc"]) or "configuration"
            details.append(f"{location}: {issue['msg']}")
        raise ConfigLoadError("; ".join(details)) from error


def load_secret(
    reference: SecretReference,
    *,
    environment: Mapping[str, str] | None = None,
    max_file_bytes: int = 1024 * 1024,
) -> bytes:
    """Resolve secret bytes without normalizing or including them in errors."""

    if max_file_bytes < 1:
        raise ValueError("max_file_bytes must be positive")
    if reference.environment is not None:
        source = os.environ if environment is None else environment
        value = source.get(reference.environment)
        if value is None or value == "":
            raise SecretLoadError(
                f"secret environment variable is missing: {reference.environment}"
            )
        return value.encode("utf-8")

    assert reference.file is not None
    try:
        size = reference.file.stat().st_size
        if size > max_file_bytes:
            raise SecretLoadError("secret file exceeds the configured size limit")
        value = reference.file.read_bytes()
    except SecretLoadError:
        raise
    except OSError as error:
        raise SecretLoadError("cannot read configured secret file") from error
    if not value:
        raise SecretLoadError("configured secret file is empty")
    return value
