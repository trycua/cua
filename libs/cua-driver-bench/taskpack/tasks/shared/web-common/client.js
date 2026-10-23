document.querySelectorAll('form').forEach((form) => {
  form.addEventListener('submit', async (event) => {
    event.preventDefault();
    const errors = form.querySelector('[role=alert]');
    errors.textContent = '';
    const data = Object.fromEntries(new FormData(form));
    if (form.dataset.shape === 'cart') {
      data.items = Object.fromEntries(
        Object.entries(data).map(([key, value]) => [key, Number(value)])
      );
      for (const key of Object.keys(data)) if (key !== 'items') delete data[key];
    }
    if (form.dataset.shape === 'address') {
      data.address = {};
      for (const key of ['name', 'street', 'city', 'postal']) {
        data.address[key] = data[key];
        delete data[key];
      }
    }
    if (data.confirmation !== undefined && data.password !== data.confirmation) {
      errors.textContent = 'confirmation: Passwords must match';
      return;
    }
    delete data.confirmation;
    try {
      const response = await fetch(form.action, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(data),
      });
      const result = await response.json();
      if (!response.ok) {
        errors.textContent = Object.entries(result.errors || {})
          .map(([key, value]) => `${key}: ${value}`)
          .join('\n');
        return;
      }
      if (form.dataset.next) {
        form.hidden = true;
        const next = document.getElementById(form.dataset.next);
        next.hidden = false;
        const review = next.querySelector('pre');
        if (review) review.textContent = JSON.stringify(result.review, null, 2);
        next.querySelector('button, input').focus();
      } else {
        errors.textContent = 'Completed successfully.';
        form.querySelector('button[type=submit]').disabled = true;
      }
    } catch {
      errors.textContent = 'Unable to submit. Please try again.';
    }
  });
});
