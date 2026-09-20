
import { useRef, useState, useCallback } from 'react';
import Input from './Input.jsx';
import './hello.css';

export default function HelloPage({ onSubmit }) {
  const [value, setValue] = useState('');
  const [error, setError] = useState(null);
  const inputRef = useRef(null);

  const focusInput = useCallback(() => inputRef.current?.focus(), []);
  const selectInput = useCallback(() => inputRef.current?.select(), []);

  const handleSubmit = useCallback(
    (e) => {
      e.preventDefault();
      const trimmed = value.trim();

      if (!trimmed) {
        setError('Username is required');
        inputRef.current?.focus();
        return;
      }
      console.log('Submitting username:', trimmed);

      onSubmit?.(trimmed);
      setValue('');
      setError(null);
      inputRef.current?.focus();
    },
    [value, onSubmit]
  );

  return (
    <form onSubmit={handleSubmit} noValidate>
      <Input
        ref={inputRef}
        label="Username"
        placeholder="Enter username"
        value={value}
        onChange={(e) => {
          setValue(e.target.value);
          if (error) setError(null);
        }}
        aria-invalid={!!error}
        aria-describedby={error ? 'username-error' : undefined}
      />

      {error && (
        <p id="username-error" role="alert" className="error">
          {error}
        </p>
      )}

      <div className="actions">
        <button type="button" onClick={focusInput}>Focus</button>
        <button type="button" onClick={selectInput}>Select All</button>
        <button type="submit">Submit</button>
      </div>
    </form>
  );
}