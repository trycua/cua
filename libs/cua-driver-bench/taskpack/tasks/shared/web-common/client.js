document.querySelectorAll('form').forEach((form) => {
  form.addEventListener('submit', async (event) => {
    event.preventDefault();
    const errors = form.querySelector('[role=alert]');
    errors.textContent = '';
    for (const input of form.querySelectorAll('[data-confirm]')) {
      if (input.value !== form.elements.namedItem(input.dataset.confirm).value) {
        errors.textContent = `${input.name}: Values must match`;
        return;
      }
    }
    const data = Object.create(null);
    for (const [name, value] of new FormData(form)) {
      const input = form.elements.namedItem(name);
      if (input.dataset.confirm) continue;
      const path = name.split('.');
      const key = path.pop();
      let target = data;
      for (const part of path) {
        target[part] ??= Object.create(null);
        target = target[part];
      }
      target[key] = input.type === 'number' ? Number(value) : value;
    }
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
