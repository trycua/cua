module.exports = (state) => {
  state.orders = [];
  state.checkout_draft = null;
  let lastConfirmed = null;
  const addressFields = ['name', 'street', 'city', 'postal'];
  const input = (name) => `<label>${name} <input name="${name}" required></label>`;
  const footer = (text, next = '') =>
    `<p role="alert" aria-live="polite"></p><button type="submit">${text}</button></form>${next}`;
  return {
    html: `<!doctype html><html lang="en"><meta charset="utf-8"><title>Pebble Checkout</title>
      <style>body{font:18px sans-serif;max-width:650px;margin:30px auto}label{display:block;margin:16px}input,button{font:inherit}[role=alert]{white-space:pre-wrap;color:#9b1020}</style>
      <h1>Pebble shop checkout</h1>
      <form action="/cart" data-shape="cart" data-next="address"><h2>1. Cart</h2>
      <label>tea <input type="number" name="tea" value="1" min="0" max="20" required></label>
      <label>cup <input type="number" name="cup" value="1" min="0" max="20" required></label>
      ${footer('Continue to address')}
      <form id="address" action="/address" data-shape="address" data-next="review" hidden><h2>2. Delivery and fake payment</h2>
      ${addressFields.map(input).join('')}${input('card')}${input('expiry')}${input('cvv')}
      ${footer('Review order')}
      <form id="review" action="/confirm" hidden><h2>3. Review</h2><pre></pre>${footer('Confirm order')}
      <script src="/client.js"></script></html>`,
    submit(route, data) {
      const errors = {};
      if (route === '/cart') {
        const items = data.items;
        if (
          !items ||
          Object.keys(items).sort().join(',') !== 'cup,tea' ||
          Object.values(items).some((q) => !Number.isInteger(q) || q < 0 || q > 20) ||
          !Object.values(items).some((q) => q > 0)
        )
          errors.items = 'Set each product quantity from 0 to 20; cart cannot be empty';
        if (Object.keys(errors).length) return { errors };
        state.checkout_draft = { items: { ...items } };
        return { accepted: true };
      }
      if (route === '/address') {
        if (!state.checkout_draft) return { errors: { cart: 'Complete cart first' } };
        for (const field of addressFields) {
          if (typeof data.address?.[field] !== 'string' || !data.address[field].trim())
            errors[field] = 'Required';
        }
        if (!/^\d{16}$/.test(data.card)) errors.card = 'Enter the sixteen-digit fake card';
        if (!/^\d{2}\/\d{2}$/.test(data.expiry)) errors.expiry = 'Use MM/YY';
        if (!/^\d{3}$/.test(data.cvv)) errors.cvv = 'Use three digits';
        if (Object.keys(errors).length) return { errors };
        Object.assign(state.checkout_draft, {
          address: Object.fromEntries(addressFields.map((field) => [field, data.address[field]])),
          card_last4: data.card.slice(-4),
        });
        return { review: state.checkout_draft };
      }
      if (route === '/confirm') {
        const draft = state.checkout_draft || lastConfirmed;
        if (!draft?.address) return { errors: { checkout: 'Complete cart and address first' } };
        state.orders.push(structuredClone(draft));
        lastConfirmed = structuredClone(draft);
        state.checkout_draft = null;
        return { confirmed: true };
      }
      return { errors: { route: 'Unknown form' } };
    },
  };
};
