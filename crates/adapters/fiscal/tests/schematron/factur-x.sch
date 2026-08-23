<?xml version="1.0" encoding="UTF-8"?>
<schema xmlns="http://purl.oclc.org/dsdl/schematron">
  <pattern id="en16931-cii">
    <rule context="CrossIndustryInvoice">
      <assert test="ExchangedDocument">ExchangedDocument is required</assert>
      <assert test="SupplyChainTradeTransaction">SupplyChainTradeTransaction is required</assert>
      <assert test="contains(.,'factur-x')">Factur-X guideline id must be present</assert>
      <assert test="contains(.,'EUR')">Document currency is required</assert>
    </rule>
  </pattern>
</schema>
