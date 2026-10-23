module.exports = (state, values) => {
  state.accounts = [];
  const fields = ['name', 'email', 'password', 'plan', 'invite'];
  const { initial } = require('./scenario.json');
  return {
    html: `<!doctype html><html lang="en"><meta charset="utf-8"><title>Fern Signup</title>
      <style>body{font:18px sans-serif;max-width:650px;margin:30px auto}label{display:block;margin:16px}input,select,button{font:inherit}[role=alert]{white-space:pre-wrap;color:#9b1020}</style>
      <h1>Fern membership</h1><form action="/signup">
      <fieldset><legend>Personal details</legend>
      <label>Name <input name="name" value="${initial.name}" required></label>
      <label>Email <input type="email" name="email" value="${initial.email}" required></label></fieldset>
      <fieldset><legend>Security</legend><label>Password <input type="password" name="password" minlength="8" required></label>
      <label>Confirm password <input type="password" name="confirmation" required></label></fieldset>
      <fieldset><legend>Membership</legend><label>Plan <select name="plan"><option>basic</option><option>plus</option></select></label>
      <label>Invitation code <input name="invite" value="${initial.invite}" required></label></fieldset>
      <p role="alert" aria-live="polite"></p><button type="submit">Create account</button></form>
      <script src="/client.js"></script></html>`,
    submit(route, data) {
      if (route !== '/signup') return { errors: { route: 'Unknown form' } };
      const errors = {};
      for (const field of fields) {
        if (typeof data[field] !== 'string' || !data[field].trim()) errors[field] = 'Required';
      }
      if (!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(data.email)) errors.email = 'Enter a valid email';
      if (typeof data.password !== 'string' || data.password.length < 8)
        errors.password = 'Use at least eight characters';
      if (!['basic', 'plus'].includes(data.plan)) errors.plan = 'Choose basic or plus';
      if (data.invite !== values.invite)
        errors.invite = 'Invitation expired. Use the current invitation from your task brief.';
      if (Object.keys(errors).length) return { errors };
      state.accounts.push(Object.fromEntries(fields.map((field) => [field, data[field]])));
      return { accepted: true };
    },
  };
};
