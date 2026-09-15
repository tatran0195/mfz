"""
RDP Launcher — Manages AWS MFA authentication, SSM port forwarding,
and optional RDP session launch.

Temporary MFA credentials are held in memory only — never written to disk.
"""

import argparse
import configparser
import logging
import os
import signal
import socket
import subprocess
import sys
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import NoReturn, Optional

import boto3
from botocore.credentials import RefreshableCredentials
from botocore.session import get_session
import pyotp

# =============================================================================
# Constants
# =============================================================================

ENVIRONMENT_CONFIG_FILE = Path("environment.txt")

MAX_SSM_RETRIES = 3
PORT_WAIT_TIMEOUT = 30
PORT_POLL_INTERVAL = 0.5
RETRY_BACKOFF_SECONDS = 3
TOTP_SAFETY_MARGIN_SECONDS = 5

REQUIRED_ENV_KEYS = frozenset({
    "PROFILE", "SERIAL", "SECRET", "INSTANCE",
    "REMOTE_PORT", "LOCAL_PORT", "RDP_HOST",
})

# =============================================================================
# Logging
# =============================================================================


def _configure_logging(log_file: str = "rdp_launcher.log") -> logging.Logger:
    _logger = logging.getLogger("rdp_launcher")
    _logger.setLevel(logging.INFO)

    if _logger.handlers:
        return _logger

    formatter = logging.Formatter("%(asctime)s [%(levelname)s] %(message)s")

    file_handler = logging.FileHandler(log_file)
    file_handler.setFormatter(formatter)

    console_handler = logging.StreamHandler(sys.stdout)
    console_handler.setFormatter(formatter)

    _logger.addHandler(file_handler)
    _logger.addHandler(console_handler)

    return _logger


logger = _configure_logging()

# =============================================================================
# Exceptions
# =============================================================================


class RdpLauncherError(Exception):
    """Base exception."""


class ConfigError(RdpLauncherError):
    """Configuration-related error."""


class SsmError(RdpLauncherError):
    """SSM session error."""


class MfaError(RdpLauncherError):
    """MFA authentication error."""

# =============================================================================
# Data Classes
# =============================================================================


@dataclass(frozen=True)
class EnvironmentConfig:
    profile: str
    serial: str
    secret: str
    instance: str
    remote_port: int
    local_port: int
    rdp_host: str

    @classmethod
    def from_config_section(
        cls, name: str, section: configparser.SectionProxy
    ) -> "EnvironmentConfig":
        missing = REQUIRED_ENV_KEYS - {k.upper() for k in section}
        if missing:
            raise ConfigError(
                f"[{name}] missing keys: {', '.join(sorted(missing))}"
            )
        try:
            remote_port = int(section["REMOTE_PORT"])
            local_port = int(section["LOCAL_PORT"])
        except ValueError as exc:
            raise ConfigError(f"Port values must be integers: {exc}") from exc

        if not (1 <= remote_port <= 65535 and 1 <= local_port <= 65535):
            raise ConfigError("Port values must be in range 1–65535")

        return cls(
            profile=section["PROFILE"],
            serial=section["SERIAL"],
            secret=section["SECRET"],
            instance=section["INSTANCE"],
            remote_port=remote_port,
            local_port=local_port,
            rdp_host=section["RDP_HOST"],
        )


@dataclass
class CLIFlags:
    environment: str
    fullscreen: bool = False
    ssm_only: bool = False

# =============================================================================
# AWS MFA Session Manager (In-Memory Only)
# =============================================================================


class AwsMfaSessionManager:
    """
    Acquires and caches AWS MFA session credentials entirely in memory.
    
    Nothing is written to ~/.aws/credentials.
    Credentials auto-refresh when they approach expiration.
    """

    def __init__(self, base_profile: str, serial: str, secret: str):
        self._base_profile = base_profile
        self._serial = serial
        self._secret = secret
        self._cached_session: Optional[boto3.Session] = None
        self._expiration: Optional[datetime] = None

    def get_session(self) -> boto3.Session:
        """Return a valid boto3 Session, refreshing MFA credentials if needed."""
        if self._is_cached_valid():
            logger.info("Reusing cached MFA session (expires %s)", self._expiration)
            return self._cached_session

        creds = self._acquire_session_token()
        self._cached_session = boto3.Session(
            aws_access_key_id=creds["AccessKeyId"],
            aws_secret_access_key=creds["SecretAccessKey"],
            aws_session_token=creds["SessionToken"],
        )
        self._expiration = creds["Expiration"]

        logger.info("New MFA session acquired (expires %s)", self._expiration)
        return self._cached_session

    def get_credentials_env(self) -> dict[str, str]:
        """
        Return credentials as environment variables for subprocess use.
        
        This is how we pass credentials to the AWS CLI subprocess
        without touching any files.
        """
        session = self.get_session()
        credentials = session.get_credentials().get_frozen_credentials()

        return {
            "AWS_ACCESS_KEY_ID": credentials.access_key,
            "AWS_SECRET_ACCESS_KEY": credentials.secret_key,
            "AWS_SESSION_TOKEN": credentials.token,
        }

    def _is_cached_valid(self) -> bool:
        if self._cached_session is None or self._expiration is None:
            return False

        # Treat as expired 60s early to avoid edge-case failures
        remaining = (self._expiration - datetime.now(timezone.utc)).total_seconds()
        if remaining < 60:
            logger.info("Cached session expiring in %.0fs — will refresh", remaining)
            return False

        # Verify credentials still work
        try:
            self._cached_session.client("sts").get_caller_identity()
            return True
        except Exception:
            logger.debug("STS verification failed for cached session")
            return False

    def _acquire_session_token(self) -> dict:
        logger.info("Requesting new AWS session token via MFA...")

        totp = pyotp.TOTP(self._secret)

        # Avoid TOTP race condition near window boundary
        remaining = totp.interval - (time.time() % totp.interval)
        if remaining < TOTP_SAFETY_MARGIN_SECONDS:
            wait_time = remaining + 0.5
            logger.info(
                "TOTP window expiring in %.1fs — waiting %.1fs", remaining, wait_time
            )
            time.sleep(wait_time)

        otp_code = totp.now()

        try:
            base_session = boto3.Session(profile_name=self._base_profile)
            response = base_session.client("sts").get_session_token(
                SerialNumber=self._serial,
                TokenCode=otp_code,
            )
        except Exception as exc:
            raise MfaError(f"Failed to obtain MFA session token: {exc}") from exc

        return response["Credentials"]

# =============================================================================
# SSM Port-Forwarding Session
# =============================================================================


class SsmSession:
    """Manages an AWS SSM port-forwarding session as a context manager."""

    def __init__(
        self,
        mfa_manager: AwsMfaSessionManager,
        instance: str,
        remote_port: int,
        local_port: int,
        region: Optional[str] = None,
    ):
        self._mfa_manager = mfa_manager
        self.instance = instance
        self.remote_port = remote_port
        self.local_port = local_port
        self._region = region
        self._process: Optional[subprocess.Popen] = None

    def __enter__(self) -> "SsmSession":
        return self

    def __exit__(self, exc_type, exc_val, exc_tb) -> None:
        self.stop()

    @property
    def is_running(self) -> bool:
        return self._process is not None and self._process.poll() is None

    def start(self) -> None:
        if self.is_running:
            logger.warning("SSM session already running — stopping first")
            self.stop()

        logger.info(
            "Starting SSM: %s -> 127.0.0.1:%d", self.instance, self.local_port
        )

        # Pass credentials via environment — no file writes needed
        env = {**os.environ, **self._mfa_manager.get_credentials_env()}

        # Remove any profile reference so CLI uses env vars exclusively
        env.pop("AWS_PROFILE", None)
        env.pop("AWS_DEFAULT_PROFILE", None)

        if self._region:
            env["AWS_DEFAULT_REGION"] = self._region

        self._process = subprocess.Popen(
            [
                "aws", "ssm", "start-session",
                "--target", self.instance,
                "--document-name", "AWS-StartPortForwardingSession",
                "--parameters",
                f"portNumber={self.remote_port},localPortNumber={self.local_port}",
            ],
            stdin=subprocess.DEVNULL,
            env=env,
        )

    def stop(self) -> None:
        if not self.is_running:
            return

        logger.info("Terminating SSM session (PID %d)", self._process.pid)
        self._process.terminate()
        try:
            self._process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            logger.warning("SSM process did not exit — killing")
            self._process.kill()
            self._process.wait(timeout=5)
        self._process = None

    def wait(self) -> int:
        if self._process is None:
            raise SsmError("No SSM process to wait on")
        return self._process.wait()

    def wait_for_port(self, timeout: int = PORT_WAIT_TIMEOUT) -> bool:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if not self.is_running:
                logger.error(
                    "SSM process exited unexpectedly (rc=%s)",
                    self._process.returncode,
                )
                return False
            if self._is_port_open():
                return True
            time.sleep(PORT_POLL_INTERVAL)
        return False

    def _is_port_open(self) -> bool:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
            sock.settimeout(1)
            return sock.connect_ex(("127.0.0.1", self.local_port)) == 0

# =============================================================================
# RDP Launcher
# =============================================================================


def launch_rdp(host: str, port: int, fullscreen: bool = False) -> None:
    cmd = ["mstsc", f"/v:{host}:{port}"]
    if fullscreen:
        cmd.append("/f")
    logger.info("Launching RDP: %s", " ".join(cmd))
    subprocess.run(cmd, check=False)
    logger.info("RDP session ended")

# =============================================================================
# CLI / Config
# =============================================================================


def parse_args() -> CLIFlags:
    parser = argparse.ArgumentParser(
        description="Launch RDP over AWS SSM with MFA authentication.",
    )
    parser.add_argument("environment", help="Environment name (e.g. stg, prd)")
    parser.add_argument("--fullscreen", "-f", action="store_true")
    parser.add_argument("--ssm-only", "-s", action="store_true")
    args = parser.parse_args()
    return CLIFlags(
        environment=args.environment,
        fullscreen=args.fullscreen,
        ssm_only=args.ssm_only,
    )


def load_environment(env_name: str) -> EnvironmentConfig:
    if not ENVIRONMENT_CONFIG_FILE.exists():
        raise ConfigError(f"Config file not found: {ENVIRONMENT_CONFIG_FILE}")
    config = configparser.ConfigParser()
    config.read(ENVIRONMENT_CONFIG_FILE)
    if env_name not in config:
        available = config.sections()
        raise ConfigError(
            f"[{env_name}] not found. Available: {', '.join(available) or '(none)'}"
        )
    return EnvironmentConfig.from_config_section(env_name, config[env_name])

# =============================================================================
# Main
# =============================================================================


def run(flags: CLIFlags) -> None:
    env = load_environment(flags.environment)
    logger.info("Starting %s session", flags.environment.upper())

    mfa_manager = AwsMfaSessionManager(
        base_profile=env.profile,
        serial=env.serial,
        secret=env.secret,
    )

    # Force initial credential acquisition before entering retry loop
    mfa_manager.get_session()

    with SsmSession(
        mfa_manager=mfa_manager,
        instance=env.instance,
        remote_port=env.remote_port,
        local_port=env.local_port,
    ) as ssm:

        def _shutdown(signum: int, frame) -> NoReturn:
            logger.info("Received %s — shutting down", signal.Signals(signum).name)
            ssm.stop()
            sys.exit(0)

        signal.signal(signal.SIGINT, _shutdown)
        signal.signal(signal.SIGTERM, _shutdown)

        connected = False
        for attempt in range(1, MAX_SSM_RETRIES + 1):
            logger.info("SSM attempt %d/%d", attempt, MAX_SSM_RETRIES)
            ssm.start()

            if ssm.wait_for_port():
                logger.info("Port forwarding ready on 127.0.0.1:%d", env.local_port)
                connected = True
                break

            logger.warning("Port forwarding failed")
            ssm.stop()

            if attempt < MAX_SSM_RETRIES:
                time.sleep(RETRY_BACKOFF_SECONDS)
                # Re-validate in case credentials expired during retries
                mfa_manager.get_session()

        if not connected:
            raise SsmError(f"SSM failed after {MAX_SSM_RETRIES} attempts")

        if flags.ssm_only:
            logger.info("SSM-only mode — press Ctrl+C to stop")
            ssm.wait()
            return

        launch_rdp(env.rdp_host, env.local_port, flags.fullscreen)


def main() -> None:
    flags = parse_args()
    try:
        run(flags)
    except (ConfigError, MfaError, SsmError) as exc:
        logger.error("%s", exc)
        sys.exit(1)
    except Exception:
        logger.exception("Unexpected error")
        sys.exit(2)


if __name__ == "__main__":
    main()