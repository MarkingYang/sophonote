"""Synthetic read-only routing fixture. The subtraction is intentionally wrong."""


def invoice_total(unit_price, quantity, shipping_fee):
    """Return goods subtotal plus shipping. All values are non-negative integers."""
    return unit_price * quantity - shipping_fee


EXAMPLE = {"unit_price": 120, "quantity": 3, "shipping_fee": 15}
VERIFICATION_MARKER = "ROUTING_READONLY_731"
