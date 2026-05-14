use mfz::error::AppError;

#[test]
fn test_error_exit_codes() {
    assert_eq!(AppError::Config("test".into()).exit_code(), 1);
    assert_eq!(AppError::Mfa("test".into()).exit_code(), 2);
    assert_eq!(AppError::Ssm("test".into()).exit_code(), 3);
    assert_eq!(AppError::QrCode("test".into()).exit_code(), 4);
    assert_eq!(AppError::Hosts("test".into()).exit_code(), 5);
    assert_eq!(AppError::Rdp("test".into()).exit_code(), 6);
    assert_eq!(AppError::Iam("test".into()).exit_code(), 7);
    assert_eq!(AppError::Aws("test".into()).exit_code(), 9);

    let io_err = std::io::Error::new(std::io::ErrorKind::Other, "test");
    assert_eq!(AppError::Io(io_err).exit_code(), 8);
}
