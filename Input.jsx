// Input.jsx
import { forwardRef, useId } from 'react';

const Input = forwardRef(function Input(
  { label, id: idProp, ...props },
  ref
) {
  const generatedId = useId();
  const id = idProp ?? generatedId;

  return (
    <div className="input-wrapper">
      {label && <label htmlFor={id}>{label}</label>}
      <input id={id} ref={ref} {...props} />
    </div>
  );
});

export default Input;