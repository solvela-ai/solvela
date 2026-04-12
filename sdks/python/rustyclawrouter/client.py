"""Solvela LLM client with transparent x402 payment handling."""

import json
import logging
from typing import List, Optional

import httpx

from .config import DEFAULT_API_URL
from .types import (
    ChatMessage,
    ChatResponse,
    CostBreakdown,
    PaymentRequired,
    Role,
)
from .wallet import Wallet
from .x402 import encode_payment_header

logger = logging.getLogger(__name__)


class PaymentError(Exception):
    """Raised when x402 payment fails or cannot be parsed."""

    pass


class BudgetExceededError(Exception):
    """Raised when session budget would be exceeded by this request."""

    pass


class LLMClient:
    """Synchronous LLM client with x402 payment support.

    Example::

        from rustyclawrouter import LLMClient

        client = LLMClient(api_url="http://localhost:8402")
        reply = client.chat("openai/gpt-4o", "Hello!")
        print(reply)
    """

    def __init__(
        self,
        private_key: Optional[str] = None,
        api_url: Optional[str] = None,
        session_budget: Optional[float] = None,
        timeout: float = 60.0,
    ):
        self.api_url = (api_url or DEFAULT_API_URL).rstrip("/")
        self.wallet = Wallet(private_key)
        self.session_budget = session_budget
        self._session_spent = 0.0
        self._client = httpx.Client(timeout=timeout)

    # --- Public helpers ---------------------------------------------------

    def chat(self, model: str, prompt: str, **kwargs) -> str:
        """Simple chat — returns just the response text."""
        response = self.chat_completion(
            model=model,
            messages=[ChatMessage(role=Role.USER, content=prompt)],
            **kwargs,
        )
        return response.choices[0].message.content

    def chat_completion(
        self,
        model: str,
        messages: List[ChatMessage],
        max_tokens: Optional[int] = None,
        temperature: Optional[float] = None,
        stream: bool = False,
        **kwargs,
    ) -> ChatResponse:
        """Full OpenAI-compatible chat completion with x402 payment handling."""
        request_body = {
            "model": model,
            "messages": [m.model_dump() for m in messages],
            "stream": stream,
        }
        if max_tokens is not None:
            request_body["max_tokens"] = max_tokens
        if temperature is not None:
            request_body["temperature"] = temperature

        url = f"{self.api_url}/v1/chat/completions"

        # First attempt — may get 402
        resp = self._client.post(url, json=request_body)

        cost = 0.0
        if resp.status_code == 402:
            payment_info = self._parse_402(resp)
            if payment_info is None:
                raise PaymentError("Failed to parse 402 response")

            cost = float(payment_info.cost_breakdown.total)
            if self.session_budget is not None and (
                self._session_spent + cost > self.session_budget
            ):
                raise BudgetExceededError(
                    f"Session budget ${self.session_budget:.4f} exceeded "
                    f"(spent: ${self._session_spent:.4f}, "
                    f"this request: ${cost:.4f})"
                )

            request_bytes = json.dumps(request_body).encode()
            payment_header = self._create_payment_header(
                payment_info, url, request_body=request_bytes
            )

            # Retry with payment
            resp = self._client.post(
                url,
                json=request_body,
                headers={"payment-signature": payment_header},
            )

        resp.raise_for_status()
        self._session_spent += cost  # Only count if request succeeded
        return ChatResponse.model_validate(resp.json())

    def smart_chat(self, prompt: str, profile: str = "auto") -> ChatResponse:
        """Smart routing — uses the gateway's smart router to pick the best model."""
        return self.chat_completion(
            model=profile,
            messages=[ChatMessage(role=Role.USER, content=prompt)],
        )

    def list_models(self) -> dict:
        """List available models and pricing."""
        resp = self._client.get(f"{self.api_url}/v1/models")
        resp.raise_for_status()
        return resp.json()

    def get_cost_estimate(
        self, model: str, input_tokens: int, output_tokens: int
    ) -> CostBreakdown:
        """Estimate cost before making a request (via 402 response)."""
        resp = self._client.post(
            f"{self.api_url}/v1/chat/completions",
            json={
                "model": model,
                "messages": [{"role": "user", "content": "cost estimate"}],
                "max_tokens": output_tokens,
            },
        )
        if resp.status_code == 402:
            info = self._parse_402(resp)
            if info:
                return info.cost_breakdown
        raise ValueError("Could not get cost estimate")

    @property
    def session_spent(self) -> float:
        """Total USDC spent in this session."""
        return self._session_spent

    def health(self) -> dict:
        """Check gateway health."""
        resp = self._client.get(f"{self.api_url}/health")
        resp.raise_for_status()
        return resp.json()

    # --- Lifecycle --------------------------------------------------------

    def close(self) -> None:
        """Close the underlying HTTP client."""
        self._client.close()

    def __enter__(self) -> "LLMClient":
        return self

    def __exit__(self, *args) -> None:
        self.close()

    # --- Internals --------------------------------------------------------

    def _parse_402(self, resp: httpx.Response) -> Optional[PaymentRequired]:
        """Parse a 402 Payment Required response."""
        try:
            body = resp.json()
            error_msg = body.get("error", {}).get("message", "")
            payment_data = json.loads(error_msg)
            return PaymentRequired.model_validate(payment_data)
        except (json.JSONDecodeError, KeyError, AttributeError) as e:
            logger.warning("Could not parse 402 response: %s", e)
            return None
        except Exception as e:
            logger.error("Unexpected error parsing 402 response: %s", e)
            return None

    def _create_payment_header(
        self,
        payment_info: PaymentRequired,
        resource_url: str,
        request_body: Optional[bytes] = None,
    ) -> str:
        """Create a PAYMENT-SIGNATURE header value.

        Prefers the escrow scheme when available. Uses real Solana signing
        when a private key and deps are available.
        """
        if not payment_info.accepts:
            raise PaymentError("Gateway returned no accepted payment methods")
        accept = next(
            (a for a in payment_info.accepts if a.scheme == "escrow" and a.escrow_program_id),
            payment_info.accepts[0],
        )
        private_key = self.wallet.private_key if self.wallet.has_key else None
        return encode_payment_header(
            accept, resource_url, private_key=private_key, request_body=request_body
        )


class AsyncLLMClient:
    """Async version of :class:`LLMClient` using ``httpx.AsyncClient``.

    Example::

        import asyncio
        from rustyclawrouter import AsyncLLMClient

        async def main():
            async with AsyncLLMClient(api_url="http://localhost:8402") as client:
                reply = await client.chat("openai/gpt-4o", "Hello!")
                print(reply)

        asyncio.run(main())
    """

    def __init__(
        self,
        private_key: Optional[str] = None,
        api_url: Optional[str] = None,
        session_budget: Optional[float] = None,
        timeout: float = 60.0,
    ):
        self.api_url = (api_url or DEFAULT_API_URL).rstrip("/")
        self.wallet = Wallet(private_key)
        self.session_budget = session_budget
        self._session_spent = 0.0
        self._client = httpx.AsyncClient(timeout=timeout)

    async def chat(self, model: str, prompt: str, **kwargs) -> str:
        """Simple chat — returns just the response text."""
        response = await self.chat_completion(
            model=model,
            messages=[ChatMessage(role=Role.USER, content=prompt)],
            **kwargs,
        )
        return response.choices[0].message.content

    async def chat_completion(
        self,
        model: str,
        messages: List[ChatMessage],
        max_tokens: Optional[int] = None,
        temperature: Optional[float] = None,
        stream: bool = False,
        **kwargs,
    ) -> ChatResponse:
        """Full OpenAI-compatible chat completion with x402 payment handling."""
        request_body = {
            "model": model,
            "messages": [m.model_dump() for m in messages],
            "stream": stream,
        }
        if max_tokens is not None:
            request_body["max_tokens"] = max_tokens
        if temperature is not None:
            request_body["temperature"] = temperature

        url = f"{self.api_url}/v1/chat/completions"
        resp = await self._client.post(url, json=request_body)

        cost = 0.0
        if resp.status_code == 402:
            payment_info = self._parse_402(resp)
            if payment_info is None:
                raise PaymentError("Failed to parse 402 response")

            cost = float(payment_info.cost_breakdown.total)
            if self.session_budget is not None and (
                self._session_spent + cost > self.session_budget
            ):
                raise BudgetExceededError(
                    f"Session budget ${self.session_budget:.4f} exceeded "
                    f"(spent: ${self._session_spent:.4f}, "
                    f"this request: ${cost:.4f})"
                )

            request_bytes = json.dumps(request_body).encode()
            payment_header = self._create_payment_header(
                payment_info, url, request_body=request_bytes
            )
            resp = await self._client.post(
                url,
                json=request_body,
                headers={"payment-signature": payment_header},
            )

        resp.raise_for_status()
        self._session_spent += cost  # Only count if request succeeded
        return ChatResponse.model_validate(resp.json())

    async def list_models(self) -> dict:
        """List available models and pricing."""
        resp = await self._client.get(f"{self.api_url}/v1/models")
        resp.raise_for_status()
        return resp.json()

    async def health(self) -> dict:
        """Check gateway health."""
        resp = await self._client.get(f"{self.api_url}/health")
        resp.raise_for_status()
        return resp.json()

    @property
    def session_spent(self) -> float:
        """Total USDC spent in this session."""
        return self._session_spent

    # --- Lifecycle --------------------------------------------------------

    async def close(self) -> None:
        """Close the underlying async HTTP client."""
        await self._client.aclose()

    async def __aenter__(self) -> "AsyncLLMClient":
        return self

    async def __aexit__(self, *args) -> None:
        await self.close()

    # --- Internals --------------------------------------------------------

    def _parse_402(self, resp: httpx.Response) -> Optional[PaymentRequired]:
        """Parse a 402 Payment Required response."""
        try:
            body = resp.json()
            error_msg = body.get("error", {}).get("message", "")
            payment_data = json.loads(error_msg)
            return PaymentRequired.model_validate(payment_data)
        except (json.JSONDecodeError, KeyError, AttributeError) as e:
            logger.warning("Could not parse 402 response: %s", e)
            return None
        except Exception as e:
            logger.error("Unexpected error parsing 402 response: %s", e)
            return None

    def _create_payment_header(
        self,
        payment_info: PaymentRequired,
        resource_url: str,
        request_body: Optional[bytes] = None,
    ) -> str:
        """Create a PAYMENT-SIGNATURE header value.

        Prefers the escrow scheme when available. Uses real Solana signing
        when a private key and deps are available.
        """
        if not payment_info.accepts:
            raise PaymentError("Gateway returned no accepted payment methods")
        accept = next(
            (a for a in payment_info.accepts if a.scheme == "escrow" and a.escrow_program_id),
            payment_info.accepts[0],
        )
        private_key = self.wallet.private_key if self.wallet.has_key else None
        return encode_payment_header(
            accept, resource_url, private_key=private_key, request_body=request_body
        )
