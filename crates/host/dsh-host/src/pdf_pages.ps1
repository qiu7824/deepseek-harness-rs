[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
try {
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Runtime.WindowsRuntime
[Windows.Storage.StorageFile,Windows.Storage,ContentType=WindowsRuntime] | Out-Null
[Windows.Data.Pdf.PdfDocument,Windows.Data.Pdf,ContentType=WindowsRuntime] | Out-Null
[Windows.Storage.Streams.InMemoryRandomAccessStream,Windows.Storage.Streams,ContentType=WindowsRuntime] | Out-Null
$asTaskGeneric=([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object {$_.Name -eq 'AsTask' -and $_.IsGenericMethodDefinition -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1'})[0]
function AwaitResult($Operation,[Type]$Type){$task=$asTaskGeneric.MakeGenericMethod($Type).Invoke($null,@($Operation));$task.GetAwaiter().GetResult()}
function AwaitAction($Action){$method=([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object {$_.Name -eq 'AsTask' -and -not $_.IsGenericMethod -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncAction'})[0];$task=$method.Invoke($null,@($Action));$task.GetAwaiter().GetResult()}
$file=AwaitResult ([Windows.Storage.StorageFile]::GetFileFromPathAsync($env:DSH_PDF_INPUT)) ([Windows.Storage.StorageFile])
$pdf=AwaitResult ([Windows.Data.Pdf.PdfDocument]::LoadFromFileAsync($file)) ([Windows.Data.Pdf.PdfDocument])
$pages=if($env:DSH_PDF_PAGES){@($env:DSH_PDF_PAGES|ConvertFrom-Json)}else{@(1)}
$rows=@()
foreach($number in $pages){
 if($number -lt 1 -or $number -gt $pdf.PageCount){throw "Page outside document: $number"}
 $page=$pdf.GetPage([uint32]($number-1));$stream=New-Object Windows.Storage.Streams.InMemoryRandomAccessStream
 try{
  $options=New-Object Windows.Data.Pdf.PdfPageRenderOptions
  $options.DestinationWidth=[uint32]1600
  $null=AwaitAction ($page.RenderToStreamAsync($stream,$options))
  $stream.Seek(0)
  $reader=New-Object Windows.Storage.Streams.DataReader($stream)
  try{
   $size=[uint32]$stream.Size
   if($size -gt 16777216){throw 'Rendered page exceeds 16 MiB'}
   $null=AwaitResult ($reader.LoadAsync($size)) ([uint32]);$bytes=New-Object byte[] $size;$reader.ReadBytes($bytes)
   $name='page-{0:D4}.png' -f $number;[IO.File]::WriteAllBytes((Join-Path $env:DSH_PDF_DIRECTORY $name),$bytes)
   $rows+=@{page=$number;file=$name;width=$options.DestinationWidth}
  }finally{if($reader){$null=$reader.DetachStream();$reader.Dispose()}}
 }finally{$stream.Dispose();$page.Dispose()}
}
@{pageCount=$pdf.PageCount;pages=$rows;renderer='Windows.Data.Pdf'} | ConvertTo-Json -Depth 5 -Compress


} catch {
 $message=[Text.Encoding]::UTF8.GetBytes($_.Exception.Message+[Environment]::NewLine)
 [Console]::OpenStandardError().Write($message,0,$message.Length)
 exit 1
}
