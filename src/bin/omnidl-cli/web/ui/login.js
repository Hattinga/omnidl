// Login: sends the password, lands on the main page with a session cookie.
'use strict';

const form = document.getElementById('login');
const input = document.getElementById('password');
const submit = document.getElementById('submit');
const error = document.getElementById('error');

input.focus();

form.addEventListener('submit', async (e) => {
  e.preventDefault();
  if (!input.value) { input.focus(); return; }
  submit.disabled = true;
  error.textContent = '';
  let message = 'Server nicht erreichbar.';
  try {
    const res = await fetch('/api/login', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', 'X-Omnidl': '1' },
      credentials: 'same-origin',
      body: JSON.stringify({ password: input.value }),
    });
    if (res.ok) {
      location.replace('/');
      return;
    }
    try { message = (await res.json()).error || message; } catch { message = `Fehler ${res.status}`; }
  } catch { /* offline */ }
  submit.disabled = false;
  error.textContent = message;
  form.classList.remove('shake');
  void form.offsetWidth;
  form.classList.add('shake');
  input.select();
});
