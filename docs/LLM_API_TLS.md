# Hosted LLM API certificate errors

`TLS certificate validation failed for <host>` means the HTTPS certificate of an LLM service could not be verified. The request stopped before any prompt or API key was sent, so it never reached the model. The certificate can belong to the service or to an HTTPS-inspection proxy, and a wrong computer clock can make a good certificate look expired.

A request to a hosted API (OpenAI, Anthropic, Google, Grok, DeepSeek, OpenRouter, APIFreeLLM, and their model lists) first checks the server against the Windows certificate store. Only if that check fails does it try the root certificates built into the app, with hostname and certificate checks still on. HTTP errors, rate limits, timeouts and other network failures are never retried. LM Studio, your own server and ComfyUI are not covered: they keep their own rules.

If `SSL_CERT_FILE` or `SSL_CERT_DIR` is set, only the certificates it names are trusted and the two stores above are skipped.

If both stores refuse the server:

1. Check the computer's date, time and time zone.
2. Update the app, which refreshes the built-in certificates, and install Windows updates, which refresh the system store.
3. Check any VPN, proxy or antivirus that inspects HTTPS. Its certificate has to be trusted by Windows (installed as a trusted root). An expired server or proxy certificate must be renewed by whoever runs it; no list of trusted certificates can make an expired one valid.
4. Try again.

The app does not turn certificate verification off and never sends an API key over a connection it could not verify. The error text names the host only, never the URL's query or a header.
